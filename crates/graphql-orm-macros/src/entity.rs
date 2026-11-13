use super::*;
use crate::backend::{
    BackendKind, backend_current_epoch_expr, backend_current_timestamp_expr,
    backend_dialect_tokens, backend_helper_import_tokens, backend_marker_tokens,
    backend_quote_identifier_path, backend_render_identifier_path, backend_row_type_tokens,
    resolve_backend,
};
use crate::naming::{graphql_field_name, selected_field_case_rule};
use crate::relationship_contract::GeneratedRelationshipArgumentContract;
use syn::spanned::Spanned;

#[derive(Default)]
pub(crate) struct EntityMetadata {
    pub(crate) stable_type_id: Option<String>,
    pub(crate) backend: Option<String>,
    pub(crate) table_name: Option<String>,
    pub(crate) plural_name: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) classification: String,
    pub(crate) default_sort: Option<String>,
    pub(crate) order_expressions: Vec<OrderExpressionMetadata>,
    pub(crate) schema_policy: Option<String>,
    pub(crate) auth: Option<String>,
    pub(crate) schema_only: bool,
    pub(crate) append_only: bool,
    /// Dedicated entity-policy key enabling host-only bounded retention purge.
    pub(crate) retention_policy: Option<String>,
    pub(crate) repository_mutations: bool,
    pub(crate) aggregate: bool,
    pub(crate) dependent_deletes: bool,
    /// Explicit AI execution policy by generated mutation category.
    pub(crate) ai_mutations: std::collections::BTreeMap<String, String>,
    pub(crate) keyset: Option<String>,
    pub(crate) query_shapes: Option<Vec<String>>,
    /// Selected generated mutation categories; `None` keeps every mutation.
    pub(crate) mutation_shapes: Option<Vec<String>>,
    pub(crate) backup_enabled: Option<bool>,
    pub(crate) backup_export_order: Option<i32>,
    pub(crate) backup_restore_order: Option<i32>,
    pub(crate) read_policy: Option<String>,
    pub(crate) write_policy: Option<String>,
    /// Optional async hook path invoked after create/update/delete mutations.
    pub(crate) notify_handler: Option<String>,
    pub(crate) notify_report_errors: bool,
    pub(crate) upsert: Option<Vec<String>>,
    pub(crate) unique_composite: Vec<Vec<String>>,
    pub(crate) indexes: Vec<OrdinaryIndexMetadata>,
    pub(crate) conditional_indexes: Vec<ConditionalIndexMetadata>,
    pub(crate) projections: Vec<ProjectionMetadata>,
    pub(crate) search: Option<SearchEntityMetadata>,
    pub(crate) serde_rename_all: Option<String>,
    pub(crate) graphql_rename_fields: Option<String>,
    pub(crate) rls: Option<RlsMetadata>,
    pub(crate) compose_complex_object: bool,
    /// Declared Apollo Federation v2 entity keys, in declaration order.
    pub(crate) federation_keys: Vec<FederationKeyMetadata>,
    /// `node_id`: generate the global `NodeId` field and its resolvable `@key`.
    pub(crate) node_id: Option<proc_macro2::Span>,
}

/// One declared `@key` for a resolvable Federation entity.
///
/// `fields` is `None` for the bare `federation_key` form, which means the
/// entity's primary key in declaration order. Otherwise it holds GraphQL field
/// names exactly as they appear in the exported schema.
#[derive(Clone)]
pub(crate) struct FederationKeyMetadata {
    pub(crate) fields: Option<Vec<String>>,
    /// Accept a key whose uniqueness the ORM cannot verify because the unique
    /// constraint lives in an externally managed schema.
    pub(crate) assume_unique: bool,
    pub(crate) span: proc_macro2::Span,
}

#[derive(Clone)]
pub(crate) struct OrderExpressionMetadata {
    pub(crate) name: String,
    pub(crate) expression: String,
    pub(crate) parameter_provider: Option<syn::Path>,
    pub(crate) parameter_names: Vec<String>,
}

#[derive(Clone, Copy)]
pub(crate) enum OrdinaryIndexDirection {
    Asc,
    Desc,
}

#[derive(Clone, Default)]
pub(crate) struct OrdinaryIndexMetadata {
    pub(crate) name: Option<String>,
    pub(crate) columns: Vec<String>,
    pub(crate) directions: Vec<OrdinaryIndexDirection>,
    pub(crate) unique: bool,
}

#[derive(Clone, Default)]
pub(crate) struct ProjectionMetadata {
    pub(crate) name: Option<String>,
    pub(crate) fields: Vec<String>,
    pub(crate) private: bool,
}

#[derive(Clone, Default)]
pub(crate) struct ConditionalIndexMetadata {
    pub(crate) name: Option<String>,
    pub(crate) columns: Vec<String>,
    pub(crate) unique: bool,
    pub(crate) predicate_field: Option<String>,
    pub(crate) predicate_values: Vec<String>,
}

#[derive(Clone)]
pub(crate) struct SearchEntityMetadata {
    pub(crate) index: bool,
    pub(crate) language: String,
    pub(crate) tokenizer: String,
    pub(crate) min_token_len: usize,
    pub(crate) fallback_enabled: bool,
}

impl Default for SearchEntityMetadata {
    fn default() -> Self {
        Self {
            index: true,
            language: "english".to_string(),
            tokenizer: "unicode61".to_string(),
            min_token_len: 2,
            fallback_enabled: true,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct RlsMetadata {
    pub(crate) force: bool,
    pub(crate) select: Option<RlsOperationMetadata>,
    pub(crate) insert: Option<RlsOperationMetadata>,
    pub(crate) update: Option<RlsOperationMetadata>,
    pub(crate) delete: Option<RlsOperationMetadata>,
}

#[derive(Clone, Default)]
pub(crate) struct RlsOperationMetadata {
    pub(crate) predicate: Option<String>,
    pub(crate) scope: Option<String>,
    pub(crate) tenant_column: Option<String>,
    pub(crate) owner_column: Option<String>,
    pub(crate) tenant_span: Option<proc_macro2::Span>,
    pub(crate) owner_span: Option<proc_macro2::Span>,
}

pub(crate) fn parse_entity_metadata(attrs: &[syn::Attribute]) -> syn::Result<EntityMetadata> {
    let mut metadata = EntityMetadata::default();
    let mut entity_attribute: Option<&'static str> = None;

    for attr in attrs {
        let attribute_name = if attr.path().is_ident("graphql_entity") {
            Some("graphql_entity")
        } else if attr.path().is_ident("repository_entity") {
            Some("repository_entity")
        } else {
            None
        };
        if let Some(attribute_name) = attribute_name {
            if let Some(previous) = entity_attribute {
                return Err(syn::Error::new_spanned(
                    attr,
                    format!(
                        "entity declarations may use only one surface attribute; found both #[{previous}] and #[{attribute_name}]"
                    ),
                ));
            }
            entity_attribute = Some(attribute_name);
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("stable_type_id") {
                    if metadata.stable_type_id.is_some() {
                        return Err(meta.error("stable_type_id may only be declared once per entity"));
                    }
                    let lit: syn::LitStr = meta.value()?.parse()?;
                    let value = lit.value();
                    if value.is_empty() || value.len() > 1024
                        || !value.bytes().all(|byte| byte.is_ascii_graphic())
                    {
                        return Err(syn::Error::new(lit.span(),
                            "stable_type_id must contain 1–1024 printable ASCII bytes without whitespace or control characters"));
                    }
                    metadata.stable_type_id = Some(value);
                } else if meta.path.is_ident("table") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    metadata.table_name = Some(lit.value());
                } else if meta.path.is_ident("backend") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    metadata.backend = Some(lit.value());
                } else if meta.path.is_ident("plural") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    metadata.plural_name = Some(lit.value());
                } else if meta.path.is_ident("description") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    validate_semantic_description(&lit.value(), lit.span())?;
                    metadata.description = Some(lit.value());
                } else if meta.path.is_ident("classification") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    validate_semantic_classification(&lit.value(), lit.span())?;
                    metadata.classification = lit.value();
                } else if meta.path.is_ident("default_sort") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    metadata.default_sort = Some(lit.value());
                } else if meta.path.is_ident("schema_policy") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    let value = lit.value();
                    validate_schema_policy(&value, lit.span())?;
                    metadata.schema_policy = Some(value);
                } else if meta.path.is_ident("auth") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    let value = lit.value();
                    validate_resolver_auth_mode(&value, lit.span())?;
                    metadata.auth = Some(value);
                } else if meta.path.is_ident("schema_only") {
                    let value = meta.value()?;
                    let lit: syn::LitBool = value.parse()?;
                    metadata.schema_only = lit.value;
                } else if meta.path.is_ident("append_only") {
                    let value = meta.value()?;
                    let lit: syn::LitBool = value.parse()?;
                    metadata.append_only = lit.value;
                } else if meta.path.is_ident("dependent_deletes") {
                    metadata.dependent_deletes = true;
                } else if meta.path.is_ident("retention_purge") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    if lit.value().trim().is_empty() {
                        return Err(syn::Error::new(
                            lit.span(),
                            "retention_purge requires a non-empty dedicated policy key",
                        ));
                    }
                    metadata.retention_policy = Some(lit.value());
                } else if meta.path.is_ident("repository_mutations") {
                    let value = meta.value()?;
                    let lit: syn::LitBool = value.parse()?;
                    metadata.repository_mutations = lit.value;
                } else if meta.path.is_ident("aggregate") {
                    let value = meta.value()?;
                    let lit: syn::LitBool = value.parse()?;
                    metadata.aggregate = lit.value;
                } else if meta.path.is_ident("ai_mutations") {
                    meta.parse_nested_meta(|operation| {
                        let Some(category) = operation.path.get_ident().map(ToString::to_string)
                        else {
                            return Err(operation.error(
                                "AI mutation category must be a simple identifier",
                            ));
                        };
                        if !matches!(
                            category.as_str(),
                            "create"
                                | "upsert"
                                | "update"
                                | "update_many"
                                | "delete"
                                | "delete_many"
                        ) {
                            return Err(operation.error(
                                "AI mutation category must be create, upsert, update, update_many, delete, or delete_many",
                            ));
                        }
                        let value = operation.value()?;
                        let lit: syn::LitStr = value.parse()?;
                        let policy = lit.value();
                        if !matches!(
                            policy.as_str(),
                            "automatic" | "approval_required" | "prohibited"
                        ) {
                            return Err(syn::Error::new(
                                lit.span(),
                                "AI mutation policy must be automatic, approval_required, or prohibited",
                            ));
                        }
                        if metadata
                            .ai_mutations
                            .insert(category.clone(), policy)
                            .is_some()
                        {
                            return Err(operation.error(format!(
                                "duplicate AI mutation category `{category}`"
                            )));
                        }
                        Ok(())
                    })?;
                } else if meta.path.is_ident("query_shapes") {
                    if attribute_name == "repository_entity" {
                        return Err(meta.error("query_shapes requires a GraphQL entity"));
                    }
                    if metadata.query_shapes.is_some() {
                        return Err(meta.error("query_shapes may only be declared once"));
                    }
                    let lit: syn::LitStr = meta.value()?.parse()?;
                    let mut shapes = Vec::new();
                    for shape in lit.value().split(',').map(str::trim) {
                        if !matches!(shape, "single" | "list" | "keyset" | "scan" | "search" | "aggregate") {
                            return Err(syn::Error::new(lit.span(), "query_shapes accepts single, list, keyset, scan, search, aggregate"));
                        }
                        if shapes.iter().any(|existing| existing == shape) {
                            return Err(syn::Error::new(lit.span(), "query_shapes must not repeat a shape"));
                        }
                        shapes.push(shape.to_owned());
                    }
                    shapes.sort();
                    metadata.query_shapes = Some(shapes);
                } else if meta.path.is_ident("mutation_shapes") {
                    if attribute_name == "repository_entity" {
                        return Err(meta.error("mutation_shapes requires a GraphQL entity"));
                    }
                    if metadata.mutation_shapes.is_some() {
                        return Err(meta.error("mutation_shapes may only be declared once"));
                    }
                    let lit: syn::LitStr = meta.value()?.parse()?;
                    let value = lit.value();
                    if value.trim().is_empty() {
                        return Err(syn::Error::new(
                            lit.span(),
                            "mutation_shapes must select at least one shape; omit it to keep every mutation, or remove the entity's mutations with schema_roots! generated_mutations",
                        ));
                    }
                    let mut shapes = Vec::new();
                    for shape in value.split(',').map(str::trim) {
                        if !matches!(
                            shape,
                            "create" | "upsert" | "update" | "update_many" | "delete" | "delete_many"
                        ) {
                            return Err(syn::Error::new(
                                lit.span(),
                                "mutation_shapes accepts create, upsert, update, update_many, delete, delete_many",
                            ));
                        }
                        if shapes.iter().any(|existing| existing == shape) {
                            return Err(syn::Error::new(
                                lit.span(),
                                "mutation_shapes must not repeat a shape",
                            ));
                        }
                        shapes.push(shape.to_owned());
                    }
                    shapes.sort();
                    metadata.mutation_shapes = Some(shapes);
                } else if meta.path.is_ident("keyset") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    metadata.keyset = Some(lit.value());
                } else if meta.path.is_ident("backup") {
                    let value = meta.value()?;
                    let lit: syn::LitBool = value.parse()?;
                    metadata.backup_enabled = Some(lit.value);
                } else if meta.path.is_ident("backup_export_order") {
                    let value = meta.value()?;
                    let lit: syn::LitInt = value.parse()?;
                    metadata.backup_export_order = Some(lit.base10_parse()?);
                } else if meta.path.is_ident("backup_restore_order") {
                    let value = meta.value()?;
                    let lit: syn::LitInt = value.parse()?;
                    metadata.backup_restore_order = Some(lit.base10_parse()?);
                } else if meta.path.is_ident("read_policy") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    metadata.read_policy = Some(lit.value());
                } else if meta.path.is_ident("write_policy") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    metadata.write_policy = Some(lit.value());
                } else if meta.path.is_ident("notify") || meta.path.is_ident("notify_with") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    metadata.notify_handler = Some(lit.value());
                } else if meta.path.is_ident("notify_on_error") {
                    let lit: syn::LitStr = meta.value()?.parse()?;
                    metadata.notify_report_errors = match lit.value().as_str() {
                        "report" => true,
                        "propagate" => false,
                        _ => return Err(syn::Error::new(lit.span(), "notify_on_error must be propagate or report")),
                    };
                } else if meta.path.is_ident("upsert") {
                    if metadata.upsert.is_some() {
                        return Err(syn::Error::new(
                            meta.path.span(),
                            "graphql_entity upsert may only be declared once per entity",
                        ));
                    }
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    let cols = lit
                        .value()
                        .split(',')
                        .map(|c| c.trim().to_string())
                        .filter(|c| !c.is_empty())
                        .collect::<Vec<_>>();
                    if cols.is_empty() {
                        return Err(syn::Error::new(
                            lit.span(),
                            "upsert must include at least one column",
                        ));
                    }
                    metadata.upsert = Some(cols);
                } else if meta.path.is_ident("node_id") {
                    if attribute_name == "repository_entity" {
                        return Err(syn::Error::new(
                            meta.path.span(),
                            "node_id requires a GraphQL entity; repository-only entities have no GraphQL object to identify",
                        ));
                    }
                    if metadata.node_id.is_some() {
                        return Err(syn::Error::new(meta.path.span(), "duplicate node_id"));
                    }
                    metadata.node_id = Some(meta.path.span());
                } else if meta.path.is_ident("federation_key") {
                    if attribute_name == "repository_entity" {
                        return Err(syn::Error::new(
                            meta.path.span(),
                            "federation_key requires a GraphQL entity; repository-only entities have no GraphQL object to resolve",
                        ));
                    }
                    let mut key = FederationKeyMetadata {
                        fields: None,
                        assume_unique: false,
                        span: meta.path.span(),
                    };
                    if meta.input.peek(syn::token::Paren) {
                        meta.parse_nested_meta(|option| {
                            if option.path.is_ident("fields") {
                                if key.fields.is_some() {
                                    return Err(syn::Error::new(
                                        option.path.span(),
                                        "federation_key accepts one fields list",
                                    ));
                                }
                                let value = option.value()?;
                                let fields = parse_string_array_expr(
                                    value,
                                    "federation_key fields must be an array of GraphQL field names",
                                )?;
                                if fields.is_empty() {
                                    return Err(syn::Error::new(
                                        option.path.span(),
                                        "federation_key fields must name at least one GraphQL field",
                                    ));
                                }
                                key.fields = Some(fields);
                            } else if option.path.is_ident("assume_unique") {
                                let value = option.value()?;
                                let lit: syn::LitBool = value.parse()?;
                                key.assume_unique = lit.value;
                            } else {
                                return Err(syn::Error::new(
                                    option.path.span(),
                                    "unsupported federation_key option; expected fields or assume_unique",
                                ));
                            }
                            Ok(())
                        })?;
                    }
                    metadata.federation_keys.push(key);
                } else if meta.path.is_ident("unique_composite") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    let cols = lit
                        .value()
                        .split(',')
                        .map(|c| c.trim().to_string())
                        .filter(|c| !c.is_empty())
                        .collect::<Vec<_>>();
                    if cols.len() < 2 {
                        return Err(syn::Error::new(
                            lit.span(),
                            "unique_composite must include at least two comma-separated columns",
                        ));
                    }
                    metadata.unique_composite.push(cols);
                } else if meta.path.is_ident("index") || meta.path.is_ident("unique_index") {
                    let unique = meta.path.is_ident("unique_index");
                    let mut index = OrdinaryIndexMetadata {
                        unique,
                        ..OrdinaryIndexMetadata::default()
                    };
                    if meta.input.peek(syn::Token![=]) {
                        let value = meta.value()?;
                        let lit: syn::LitStr = value.parse()?;
                        index.columns = lit
                            .value()
                            .split(',')
                            .map(|c| c.trim().to_string())
                            .filter(|c| !c.is_empty())
                            .collect();
                    } else {
                        meta.parse_nested_meta(|option| {
                            if option.path.is_ident("name") {
                                let value = option.value()?;
                                let lit: syn::LitStr = value.parse()?;
                                index.name = Some(lit.value());
                            } else if option.path.is_ident("columns") {
                                let value = option.value()?;
                                index.columns = parse_string_array_expr(
                                    value,
                                    "index columns must be an array of field names",
                                )?;
                            } else if option.path.is_ident("directions") {
                                let value = option.value()?;
                                let directions = parse_string_array_expr(
                                    value,
                                    "index directions must be an array containing asc or desc",
                                )?;
                                index.directions = directions
                                    .into_iter()
                                    .map(|direction| {
                                        match direction.to_ascii_lowercase().as_str() {
                                            "asc" => Ok(OrdinaryIndexDirection::Asc),
                                            "desc" => Ok(OrdinaryIndexDirection::Desc),
                                            _ => Err(syn::Error::new(
                                                option.path.span(),
                                                "index directions must contain only asc or desc",
                                            )),
                                        }
                                    })
                                    .collect::<syn::Result<Vec<_>>>()?;
                            } else {
                                return Err(syn::Error::new(
                                    option.path.span(),
                                    "unsupported ordinary index option",
                                ));
                            }
                            Ok(())
                        })?;
                    }
                    if index.columns.is_empty() {
                        return Err(syn::Error::new(
                            meta.path.span(),
                            "index must include at least one column",
                        ));
                    }
                    if index
                        .name
                        .as_deref()
                        .is_some_and(|name| name.trim().is_empty())
                    {
                        return Err(syn::Error::new(
                            meta.path.span(),
                            "index name must not be empty",
                        ));
                    }
                    if !index.directions.is_empty() && index.directions.len() != index.columns.len()
                    {
                        return Err(syn::Error::new(
                            meta.path.span(),
                            "index directions must have the same arity as index columns",
                        ));
                    }
                    metadata.indexes.push(index);
                }
                Ok(())
            })?;
        } else if attr.path().is_ident("graphql_rls") {
            if metadata.rls.is_some() {
                return Err(syn::Error::new(
                    attr.span(),
                    "graphql_rls may only be declared once per entity",
                ));
            }
            metadata.rls = Some(parse_rls_metadata(attr)?);
        } else if attr.path().is_ident("graphql_orm") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("compose_complex_object") {
                    metadata.compose_complex_object = true;
                } else if meta.path.is_ident("search") {
                    let mut search = SearchEntityMetadata::default();
                    meta.parse_nested_meta(|search_meta| {
                        if search_meta.path.is_ident("index") {
                            let value = search_meta.value()?;
                            let lit: syn::LitBool = value.parse()?;
                            search.index = lit.value;
                        } else if search_meta.path.is_ident("language") {
                            let value = search_meta.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            search.language = lit.value();
                        } else if search_meta.path.is_ident("tokenizer") {
                            let value = search_meta.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            search.tokenizer = lit.value();
                        } else if search_meta.path.is_ident("min_token_len") {
                            let value = search_meta.value()?;
                            let lit: syn::LitInt = value.parse()?;
                            search.min_token_len = lit.base10_parse()?;
                        } else if search_meta.path.is_ident("fallback") {
                            let value = search_meta.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            match lit.value().as_str() {
                                "enabled" => search.fallback_enabled = true,
                                "disabled" => search.fallback_enabled = false,
                                _ => {
                                    return Err(syn::Error::new(
                                        lit.span(),
                                        "search fallback must be \"enabled\" or \"disabled\"",
                                    ));
                                }
                            }
                        } else {
                            return Err(syn::Error::new(
                                search_meta.path.span(),
                                "unsupported search option; expected index, language, tokenizer, min_token_len, or fallback",
                            ));
                        }
                        Ok(())
                    })?;
                    metadata.search = Some(search);
                } else if meta.path.is_ident("order_expression") {
                    let mut name = None;
                    let mut expression = None;
                    let mut parameter_provider = None;
                    meta.parse_nested_meta(|option| {
                        if option.path.is_ident("name") {
                            let value = option.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            validate_graphql_order_name(&lit.value(), lit.span())?;
                            name = Some(lit.value());
                        } else if option.path.is_ident("expression") {
                            let value = option.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            validate_order_expression(&lit.value(), lit.span())?;
                            expression = Some(lit.value());
                        } else if option.path.is_ident("parameters") {
                            let value = option.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            parameter_provider = Some(syn::parse_str::<syn::Path>(&lit.value()).map_err(
                                |_| {
                                    syn::Error::new(
                                        lit.span(),
                                        "order_expression parameters must be a Rust function path",
                                    )
                                },
                            )?);
                        } else {
                            return Err(syn::Error::new(
                                option.path.span(),
                                "unsupported order_expression option; expected name, expression, or parameters",
                            ));
                        }
                        Ok(())
                    })?;
                    let name = name.ok_or_else(|| {
                            syn::Error::new(
                                meta.path.span(),
                                "order_expression requires name = \"GraphQLField\"",
                            )
                        })?;
                    let expression = expression.ok_or_else(|| {
                            syn::Error::new(
                                meta.path.span(),
                                "order_expression requires expression = \"trusted SQL expression\"",
                            )
                        })?;
                    let (expression, parameter_names) =
                        rewrite_named_order_parameters(&expression, meta.path.span())?;
                    if !parameter_names.is_empty() && parameter_provider.is_none() {
                        return Err(syn::Error::new(
                            meta.path.span(),
                            "order_expression named parameters require parameters = \"server_function_path\"",
                        ));
                    }
                    if parameter_names.is_empty() && parameter_provider.is_some() {
                        return Err(syn::Error::new(
                            meta.path.span(),
                            "order_expression parameters requires at least one :named_parameter in expression",
                        ));
                    }
                    metadata.order_expressions.push(OrderExpressionMetadata {
                        name,
                        expression,
                        parameter_provider,
                        parameter_names,
                    });
                } else if meta.path.is_ident("conditional_index") {
                    let mut index = ConditionalIndexMetadata::default();
                    meta.parse_nested_meta(|option| {
                        if option.path.is_ident("name") {
                            let value = option.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            index.name = Some(lit.value());
                        } else if option.path.is_ident("columns") {
                            let value = option.value()?;
                            index.columns = parse_string_array_expr(
                                value,
                                "conditional_index columns must be an array of field names",
                            )?;
                        } else if option.path.is_ident("unique") {
                            let value = option.value()?;
                            let lit: syn::LitBool = value.parse()?;
                            index.unique = lit.value;
                        } else if option.path.is_ident("predicate_field") {
                            let value = option.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            index.predicate_field = Some(lit.value());
                        } else if option.path.is_ident("predicate_values") {
                            let value = option.value()?;
                            index.predicate_values = parse_string_array_expr(
                                value,
                                "conditional_index predicate_values must be an array of string literals",
                            )?;
                        } else {
                            return Err(syn::Error::new(
                                option.path.span(),
                                "unsupported conditional_index option",
                            ));
                        }
                        Ok(())
                    })?;
                    if index.columns.is_empty()
                        || index.predicate_field.is_none()
                        || index.predicate_values.is_empty()
                    {
                        return Err(syn::Error::new(
                            meta.path.span(),
                            "conditional_index requires columns, predicate_field, and predicate_values",
                        ));
                    }
                    metadata.conditional_indexes.push(index);
                } else if meta.path.is_ident("projection") {
                    let mut projection = ProjectionMetadata {
                        private: true,
                        ..ProjectionMetadata::default()
                    };
                    meta.parse_nested_meta(|option| {
                        if option.path.is_ident("name") {
                            let value = option.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            projection.name = Some(lit.value());
                        } else if option.path.is_ident("fields") {
                            let value = option.value()?;
                            projection.fields = parse_projection_field_array(
                                value,
                            )?;
                        } else if option.path.is_ident("private") {
                            let value = option.value()?;
                            let lit: syn::LitBool = value.parse()?;
                            projection.private = lit.value;
                        } else {
                            return Err(syn::Error::new(
                                option.path.span(),
                                "unsupported projection option; expected name, fields, or private",
                            ));
                        }
                        Ok(())
                    })?;
                    if projection.name.is_none() {
                        return Err(syn::Error::new(
                            meta.path.span(),
                            "projection requires name = \"TypeName\"",
                        ));
                    }
                    if projection.fields.is_empty() {
                        return Err(syn::Error::new(
                            meta.path.span(),
                            "projection requires at least one field",
                        ));
                    }
                    if !projection.private {
                        return Err(syn::Error::new(
                            meta.path.span(),
                            "public GraphQL projections are not supported; use private = true",
                        ));
                    }
                    metadata.projections.push(projection);
                } else if meta.path.is_ident("operation_authorization") {
                    // `GraphQLOperations` owns this declaration. Consume its
                    // expression-valued options here so deriving
                    // `GraphQLEntity` alongside it remains order-independent.
                    meta.parse_nested_meta(|option| {
                        let value = option.value()?;
                        let _: syn::Expr = value.parse()?;
                        Ok(())
                    })?;
                }
                Ok(())
            })?;
        } else if attr.path().is_ident("graphql") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename_fields") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    metadata.graphql_rename_fields = Some(lit.value());
                    Ok(())
                } else {
                    // async-graphql owns the other object options (guards, complex, ...).
                    skip_foreign_option(&meta)
                }
            })?;
        } else if attr.path().is_ident("serde") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename_all") && meta.input.peek(syn::Token![=]) {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    metadata.serde_rename_all = Some(lit.value());
                    Ok(())
                } else {
                    skip_foreign_option(&meta)
                }
            })?;
        }
    }

    if metadata.append_only && metadata.upsert.is_some() {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "append_only entities cannot declare upsert",
        ));
    }
    if metadata.retention_policy.is_some() && !metadata.append_only {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "retention_purge is valid only for append_only entities",
        ));
    }
    if metadata.append_only
        && metadata
            .rls
            .as_ref()
            .is_some_and(|rls| rls.update.is_some() || rls.delete.is_some())
    {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "append_only entities cannot declare UPDATE or DELETE RLS policies",
        ));
    }

    Ok(metadata)
}

/// PostgreSQL's identifier limit in bytes. Longer names are truncated by the
/// server, so generated names that would exceed it are shortened here.
const MAX_GENERATED_IDENTIFIER_BYTES: usize = 63;

/// Fixed GraphQL name under which a managed record version is also served;
/// mirrors `graphql_orm::graphql::orm::RECORD_VERSION_FIELD`.
const RECORD_VERSION_FIELD: &str = "RecordVersion";

/// Bounds a generated physical identifier to 63 bytes.
///
/// Names that already fit are returned unchanged. Longer names keep a 46-byte
/// prefix (on a character boundary) followed by `_` and the 16-digit FNV-1a
/// hash of the complete name, matching the runtime's generated index,
/// foreign-key and check-constraint names. The result is deterministic, so
/// metadata, DDL and introspection agree on every backend.
fn bounded_generated_identifier(full: String) -> String {
    if full.len() <= MAX_GENERATED_IDENTIFIER_BYTES {
        return full;
    }
    let mut hash = 0xcbf29ce484222325u64;
    for byte in full.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let mut prefix_len = MAX_GENERATED_IDENTIFIER_BYTES - 17;
    while !full.is_char_boundary(prefix_len) {
        prefix_len -= 1;
    }
    format!("{}_{hash:016x}", &full[..prefix_len])
}

fn validate_graphql_order_name(value: &str, span: proc_macro2::Span) -> syn::Result<()> {
    let mut chars = value.chars();
    let valid_start = chars
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic());
    if !valid_start
        || !chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
        || value.starts_with("__")
    {
        return Err(syn::Error::new(
            span,
            "order_expression name must be a non-reserved GraphQL name",
        ));
    }
    Ok(())
}

fn validate_order_expression(value: &str, span: proc_macro2::Span) -> syn::Result<()> {
    if value.is_empty()
        || value.trim() != value
        || value.len() > 4096
        || value.chars().any(char::is_control)
        || value.contains(';')
        || value.contains("--")
        || value.contains("/*")
        || value.contains("*/")
    {
        return Err(syn::Error::new(
            span,
            "order_expression must be one trimmed, non-empty SQL expression without comments, statement separators, or control characters",
        ));
    }
    Ok(())
}

fn rewrite_named_order_parameters(
    value: &str,
    span: proc_macro2::Span,
) -> syn::Result<(String, Vec<String>)> {
    let chars = value.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(value.len());
    let mut names = Vec::new();
    let mut index = 0usize;
    let mut quote = None;
    while index < chars.len() {
        let character = chars[index];
        if let Some(terminator) = quote {
            output.push(character);
            if character == terminator {
                if index + 1 < chars.len() && chars[index + 1] == terminator {
                    output.push(chars[index + 1]);
                    index += 2;
                    continue;
                }
                quote = None;
            }
            index += 1;
            continue;
        }
        if matches!(character, '\'' | '"') {
            quote = Some(character);
            output.push(character);
            index += 1;
            continue;
        }
        if character == '[' {
            quote = Some(']');
            output.push(character);
            index += 1;
            continue;
        }
        if matches!(character, '?' | '$' | '@') {
            return Err(syn::Error::new(
                span,
                "order_expression bind placeholders must use :name and a server parameter provider",
            ));
        }
        if character == ':' {
            if index + 1 < chars.len() && chars[index + 1] == ':' {
                output.push(':');
                output.push(':');
                index += 2;
                continue;
            }
            if index + 1 < chars.len()
                && (chars[index + 1] == '_' || chars[index + 1].is_ascii_alphabetic())
            {
                let start = index + 1;
                let mut end = start + 1;
                while end < chars.len() && (chars[end] == '_' || chars[end].is_ascii_alphanumeric())
                {
                    end += 1;
                }
                names.push(chars[start..end].iter().collect());
                output.push('?');
                index = end;
                continue;
            }
        }
        output.push(character);
        index += 1;
    }
    Ok((output, names))
}

#[cfg(test)]
mod order_expression_tests {
    use super::{
        rewrite_named_order_parameters, validate_graphql_order_name, validate_order_expression,
    };

    #[test]
    fn accepts_one_server_defined_expression() {
        validate_graphql_order_name("Duration", proc_macro2::Span::call_site()).unwrap();
        validate_order_expression(
            "COALESCE(finished_at, started_at) - started_at",
            proc_macro2::Span::call_site(),
        )
        .unwrap();
    }

    #[test]
    fn rejects_reserved_names_and_multi_statement_sql() {
        assert!(validate_graphql_order_name("__duration", proc_macro2::Span::call_site()).is_err());
        assert!(
            validate_order_expression(
                "finished_at; DROP TABLE jobs",
                proc_macro2::Span::call_site()
            )
            .is_err()
        );
        assert!(
            validate_order_expression(
                "finished_at /* request-controlled */",
                proc_macro2::Span::call_site()
            )
            .is_err()
        );
    }

    #[test]
    fn rewrites_only_named_server_parameters() {
        let (sql, names) = rewrite_named_order_parameters(
            "COALESCE(exit_time, :as_of) - entry_time + :offset",
            proc_macro2::Span::call_site(),
        )
        .unwrap();
        assert_eq!(sql, "COALESCE(exit_time, ?) - entry_time + ?");
        assert_eq!(names, ["as_of", "offset"]);

        let (sql, names) = rewrite_named_order_parameters(
            "payload::text || ':literal'",
            proc_macro2::Span::call_site(),
        )
        .unwrap();
        assert_eq!(sql, "payload::text || ':literal'");
        assert!(names.is_empty());
        assert!(
            rewrite_named_order_parameters("exit_time - ?", proc_macro2::Span::call_site())
                .is_err()
        );
    }
}

pub(crate) fn has_repository_entity_attribute(attrs: &[syn::Attribute]) -> bool {
    attrs
        .iter()
        .any(|attribute| attribute.path().is_ident("repository_entity"))
}

pub(crate) fn has_graphql_entity_attribute(attrs: &[syn::Attribute]) -> bool {
    attrs
        .iter()
        .any(|attribute| attribute.path().is_ident("graphql_entity"))
}

fn parse_rls_metadata(attr: &syn::Attribute) -> syn::Result<RlsMetadata> {
    let mut rls = RlsMetadata {
        force: true,
        ..RlsMetadata::default()
    };

    if matches!(&attr.meta, syn::Meta::Path(_)) {
        return Ok(rls);
    }

    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("force") {
            let value = meta.value()?;
            let lit: syn::LitBool = value.parse()?;
            rls.force = lit.value;
            return Ok(());
        }

        let target = if meta.path.is_ident("select") {
            &mut rls.select
        } else if meta.path.is_ident("insert") {
            &mut rls.insert
        } else if meta.path.is_ident("update") {
            &mut rls.update
        } else if meta.path.is_ident("delete") {
            &mut rls.delete
        } else {
            return Err(syn::Error::new(
                meta.path.span(),
                "unsupported graphql_rls option; expected force, select, insert, update, or delete",
            ));
        };

        if target.is_some() {
            return Err(syn::Error::new(
                meta.path.span(),
                "graphql_rls operation may only be declared once",
            ));
        }

        let mut operation = RlsOperationMetadata::default();
        meta.parse_nested_meta(|op| {
            let value = op.value()?;
            let lit: syn::LitStr = value.parse()?;
            if op.path.is_ident("predicate") {
                operation.predicate = Some(lit.value());
            } else if op.path.is_ident("scope") {
                operation.scope = Some(lit.value());
            } else if op.path.is_ident("tenant") {
                operation.tenant_column = Some(lit.value());
                operation.tenant_span = Some(lit.span());
            } else if op.path.is_ident("owner") {
                operation.owner_column = Some(lit.value());
                operation.owner_span = Some(lit.span());
            } else {
                return Err(syn::Error::new(
                    op.path.span(),
                    "unsupported graphql_rls operation option; expected predicate, scope, tenant, or owner",
                ));
            }
            Ok(())
        })?;

        if operation.predicate.is_some()
            && (operation.scope.is_some()
                || operation.tenant_column.is_some()
                || operation.owner_column.is_some())
        {
            return Err(syn::Error::new(
                meta.path.span(),
                "graphql_rls predicate cannot be combined with scope, tenant, or owner on the same operation",
            ));
        }

        *target = Some(operation);
        Ok(())
    })?;

    Ok(rls)
}

pub(crate) fn validate_schema_policy(value: &str, span: proc_macro2::Span) -> syn::Result<()> {
    match value {
        "external_read_only" | "external_writable" | "validate_only" | "plan_only" | "managed" => {
            Ok(())
        }
        _ => Err(syn::Error::new(
            span,
            "schema_policy must be one of \"external_read_only\", \"external_writable\", \"validate_only\", \"plan_only\", or \"managed\"",
        )),
    }
}

pub(crate) fn schema_policy_tokens(policy: Option<&str>) -> proc_macro2::TokenStream {
    match policy {
        Some("external_read_only") => {
            quote! { Some(::graphql_orm::graphql::orm::SchemaPolicy::ExternalReadOnly) }
        }
        Some("external_writable") => {
            quote! { Some(::graphql_orm::graphql::orm::SchemaPolicy::ExternalWritable) }
        }
        Some("validate_only") => {
            quote! { Some(::graphql_orm::graphql::orm::SchemaPolicy::ValidateOnly) }
        }
        Some("plan_only") => {
            quote! { Some(::graphql_orm::graphql::orm::SchemaPolicy::PlanOnly) }
        }
        Some("managed") => quote! { Some(::graphql_orm::graphql::orm::SchemaPolicy::Managed) },
        _ => quote! { None },
    }
}

pub(crate) fn validate_resolver_auth_mode(value: &str, span: proc_macro2::Span) -> syn::Result<()> {
    match value {
        "required" | "optional" | "none" => Ok(()),
        _ => Err(syn::Error::new(
            span,
            "auth must be one of \"required\", \"optional\", or \"none\"",
        )),
    }
}

pub(crate) fn resolver_auth_mode_tokens(
    auth: Option<&str>,
    span: proc_macro2::Span,
) -> syn::Result<proc_macro2::TokenStream> {
    match auth {
        Some("required") => Ok(quote! {
            Some(::graphql_orm::graphql::auth::ResolverAuthMode::Required)
        }),
        Some("optional") => Ok(quote! {
            Some(::graphql_orm::graphql::auth::ResolverAuthMode::Optional)
        }),
        Some("none") => Ok(quote! {
            Some(::graphql_orm::graphql::auth::ResolverAuthMode::None)
        }),
        None => Ok(quote! { None }),
        Some(value) => Err(syn::Error::new(
            span,
            format!(
                "unsupported auth mode `{value}`; expected \"required\", \"optional\", or \"none\""
            ),
        )),
    }
}

pub(crate) fn resolver_auth_mode_value_tokens(
    auth: &str,
    span: proc_macro2::Span,
) -> syn::Result<proc_macro2::TokenStream> {
    match auth {
        "required" => Ok(quote! {
            ::graphql_orm::graphql::auth::ResolverAuthMode::Required
        }),
        "optional" => Ok(quote! {
            ::graphql_orm::graphql::auth::ResolverAuthMode::Optional
        }),
        "none" => Ok(quote! {
            ::graphql_orm::graphql::auth::ResolverAuthMode::None
        }),
        value => Err(syn::Error::new(
            span,
            format!(
                "unsupported auth mode `{value}`; expected \"required\", \"optional\", or \"none\""
            ),
        )),
    }
}

fn option_lit_tokens(value: Option<&str>, span: proc_macro2::Span) -> proc_macro2::TokenStream {
    value
        .map(|value| {
            let lit = syn::LitStr::new(value, span);
            quote! { Some(#lit) }
        })
        .unwrap_or_else(|| quote! { None })
}

/// Require every `tenant` and `owner` of `#[graphql_rls]` to name a stored
/// column of the entity, so generated policies compare a real column.
fn validate_rls_columns<'a>(
    rls: &RlsMetadata,
    fields: impl IntoIterator<Item = &'a Field>,
) -> syn::Result<()> {
    let mut columns = Vec::new();
    for field in fields {
        let meta = parse_field_metadata(field)?;
        if meta.is_relation || meta.skip_db {
            continue;
        }
        let Some(ident) = field.ident.as_ref() else {
            continue;
        };
        columns.push(meta.db_column.clone().unwrap_or_else(|| ident.to_string()));
    }
    for operation in [&rls.select, &rls.insert, &rls.update, &rls.delete]
        .into_iter()
        .flatten()
    {
        for (option, column, span) in [
            ("tenant", &operation.tenant_column, operation.tenant_span),
            ("owner", &operation.owner_column, operation.owner_span),
        ] {
            let (Some(column), Some(span)) = (column, span) else {
                continue;
            };
            if !columns.iter().any(|candidate| candidate == column) {
                return Err(syn::Error::new(
                    span,
                    format!(
                        "graphql_rls {option} = \"{column}\" must name a stored column of this entity; expected one of: {}",
                        columns.join(", ")
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn rls_operation_policy_tokens(
    operation: proc_macro2::TokenStream,
    policy: &RlsOperationMetadata,
    span: proc_macro2::Span,
) -> proc_macro2::TokenStream {
    if let Some(predicate) = policy.predicate.as_deref() {
        let lit = syn::LitStr::new(predicate, span);
        return quote! {
            ::graphql_orm::graphql::orm::RlsOperationPolicy::custom(#operation, #lit)
        };
    }

    let scope = option_lit_tokens(policy.scope.as_deref(), span);
    let tenant = option_lit_tokens(policy.tenant_column.as_deref(), span);
    let owner = option_lit_tokens(policy.owner_column.as_deref(), span);
    quote! {
        ::graphql_orm::graphql::orm::RlsOperationPolicy::generated(
            #operation,
            #scope,
            #tenant,
            #owner,
        )
    }
}

fn rls_impl_tokens(
    struct_name: &syn::Ident,
    entity_name_lit: &str,
    table_name: &str,
    rls: Option<&RlsMetadata>,
    retention_purge: bool,
) -> proc_macro2::TokenStream {
    let Some(rls) = rls else {
        return quote! {
            impl ::graphql_orm::graphql::orm::DatabaseRls for #struct_name {}
        };
    };

    let span = struct_name.span();
    let entity_name = syn::LitStr::new(entity_name_lit, span);
    let table_name = syn::LitStr::new(table_name, span);
    let force = rls.force;
    let mut policies = Vec::new();
    for (operation, policy) in [
        (
            quote! { ::graphql_orm::graphql::orm::RlsOperation::Select },
            rls.select.as_ref(),
        ),
        (
            quote! { ::graphql_orm::graphql::orm::RlsOperation::Insert },
            rls.insert.as_ref(),
        ),
        (
            quote! { ::graphql_orm::graphql::orm::RlsOperation::Update },
            rls.update.as_ref(),
        ),
        (
            quote! { ::graphql_orm::graphql::orm::RlsOperation::Delete },
            rls.delete.as_ref(),
        ),
    ] {
        if let Some(policy) = policy {
            policies.push(rls_operation_policy_tokens(operation, policy, span));
        }
    }
    if retention_purge {
        let predicate = syn::LitStr::new(
            &format!(
                "current_setting('graphql_orm.retention_entity', true) = '{}'",
                table_name.value().replace('\'', "''")
            ),
            span,
        );
        policies.push(quote! {
            ::graphql_orm::graphql::orm::RlsOperationPolicy::custom(
                ::graphql_orm::graphql::orm::RlsOperation::Delete,
                #predicate,
            )
        });
    }

    quote! {
        impl ::graphql_orm::graphql::orm::DatabaseRls for #struct_name {
            fn rls_metadata() -> Option<&'static ::graphql_orm::graphql::orm::RlsEntityMetadata> {
                static POLICIES: &[::graphql_orm::graphql::orm::RlsOperationPolicy] = &[
                    #(#policies),*
                ];
                static METADATA: ::std::sync::OnceLock<::graphql_orm::graphql::orm::RlsEntityMetadata> =
                    ::std::sync::OnceLock::new();
                Some(METADATA.get_or_init(|| {
                    ::graphql_orm::graphql::orm::RlsEntityMetadata {
                        entity_name: #entity_name,
                        table_name: #table_name,
                        force: #force,
                        policies: POLICIES,
                    }
                }))
            }
        }
    }
}

/// Whether `graphql_policy_object` was applied to this struct. It runs before
/// the derives and leaves the inert `graphql_orm_policy_object` helper marker.
pub(crate) fn has_policy_object_marker(attrs: &[syn::Attribute]) -> bool {
    attrs
        .iter()
        .any(|attr| attr.path().is_ident("graphql_orm_policy_object"))
}

/// Check the effective SimpleObject skip independently of the legacy ORM
/// input/filter exclusion metadata.
pub(crate) fn has_graphql_object_skip(field: &Field) -> bool {
    field.attrs.iter().any(|attr| {
        attr.path().is_ident("graphql")
            && attr
                .parse_args_with(
                    syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
                )
                .is_ok_and(|options| {
                    options.iter().any(|option| match option {
                        syn::Meta::Path(path) => path.is_ident("skip"),
                        syn::Meta::NameValue(value) if value.path.is_ident("skip") => {
                            if let syn::Expr::Lit(literal) = &value.value {
                                matches!(&literal.lit, syn::Lit::Bool(value) if value.value)
                            } else {
                                false
                            }
                        }
                        _ => false,
                    })
                })
    })
}

/// Persisted scalar conversion must follow the owner of the output getter.
/// ORM-owned getters ignore SimpleObject output flags; delegated and metadata-only
/// objects must preserve them. Custom visibility/guards cannot be copied to the IR.
fn scalar_output_is_readable(
    field: &Field,
    metadata: &FieldMetadata,
    owns_output: bool,
    object_attrs: &[syn::Attribute],
) -> bool {
    if !metadata.graphql_readable() || metadata.read_policy.is_some() {
        return false;
    }
    if owns_output {
        return true;
    }
    let restricted = |attrs: &[syn::Attribute], scalar: bool| {
        attrs
            .iter()
            .filter(|attr| attr.path().is_ident("graphql"))
            .any(|attr| {
                attr.parse_args_with(
                    syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
                )
                .map_or(true, |options| {
                    options.iter().any(|option| {
                        let path = option.path();
                        // Dynamic visibility or guards are not runtime authority grants.
                        // Refuse rather than silently stripping those static constraints.
                        if path.is_ident("guard") {
                            return true;
                        }
                        let boolean_constraint = path.is_ident("visible")
                            || (scalar
                                && ["skip", "skip_output", "flatten", "inaccessible"]
                                    .iter()
                                    .any(|name| path.is_ident(name)));
                        if !boolean_constraint {
                            return false;
                        }
                        let boolean = match option {
                            syn::Meta::Path(_) => Some(true),
                            syn::Meta::NameValue(value) => match &value.value {
                                syn::Expr::Lit(literal) => match &literal.lit {
                                    syn::Lit::Bool(value) => Some(value.value),
                                    _ => None,
                                },
                                _ => None,
                            },
                            _ => None,
                        };
                        if path.is_ident("visible") {
                            boolean != Some(true)
                        } else {
                            boolean != Some(false)
                        }
                    })
                })
            })
    };
    !restricted(&field.attrs, true) && !restricted(object_attrs, false)
}

// Resolve target references as generated SQL does. Metadata column names are
// physical names; PostgreSQL folds only unquoted SQL references.
fn relation_column_identity(backend: BackendKind, name: &str, reference: bool) -> String {
    let rendered = reference.then(|| backend_render_identifier_path(backend, name));
    let name = rendered.as_deref().map_or(name, |rendered| {
        crate::backend::identifier_path_parts(rendered)
            .last()
            .unwrap_or("")
    });
    if name.len() >= 2 {
        if name.starts_with('"') && name.ends_with('"') {
            return name[1..name.len() - 1].replace("\"\"", "\"");
        }
        if backend != BackendKind::Postgres && name.starts_with('[') && name.ends_with(']') {
            return name[1..name.len() - 1].replace("]]", "]");
        }
        if backend == BackendKind::Sqlite && name.starts_with('`') && name.ends_with('`') {
            return name[1..name.len() - 1].replace("``", "`");
        }
    }
    if reference && backend == BackendKind::Postgres {
        name.to_ascii_lowercase()
    } else {
        name.to_owned()
    }
}

/// Refuse generated relation reads that disclose statically unreadable keys.
/// Validate local and self-target columns available in this derive's input AST.
pub(crate) fn validate_graphql_relation_keys(
    backend: BackendKind,
    entity: &syn::Ident,
    field: &Field,
    metadata: &FieldMetadata,
    fields: &[ParsedField],
    source_columns: &[String],
    target_columns: &[String],
) -> syn::Result<()> {
    if !metadata.graphql_readable() {
        return Ok(());
    }
    let check = |columns: &[String], side: &str, physical: bool| -> syn::Result<()> {
        for column in columns {
            let target_name = physical.then(|| relation_column_identity(backend, column, true));
            let mut matched = false;
            for key in fields.iter().filter(|parsed| {
                if physical && (parsed.meta.is_relation || parsed.meta.skip_db) {
                    return false;
                }
                parsed.field.ident.as_ref().is_some_and(|name| {
                    if let Some(target_name) = &target_name {
                        let rust_name = name.to_string();
                        let physical_name = parsed.meta.db_column.as_deref().unwrap_or(&rust_name);
                        let physical_name = relation_column_identity(backend, physical_name, false);
                        if backend == BackendKind::Postgres {
                            physical_name == *target_name
                        } else {
                            physical_name.eq_ignore_ascii_case(target_name)
                        }
                    } else {
                        name == column
                    }
                })
            }) {
                matched = true;
                if !key.meta.graphql_readable() {
                    return Err(syn::Error::new_spanned(
                        field,
                        format!(
                            "readable relation uses unreadable {side} `{column}`; make the relation unreadable too with #[graphql_orm(read = false)], or remove private/input_only and set read = true on the key"
                        ),
                    ));
                }
            }
            if physical && !matched {
                return Err(syn::Error::new_spanned(
                    field,
                    format!(
                        "cannot resolve {side} `{column}` to a declared database column; use its physical column name with matching SQL quoting"
                    ),
                ));
            }
        }
        Ok(())
    };
    check(source_columns, "local key", false)?;
    if let Some(condition) = &metadata.relation_source_condition {
        check(
            std::slice::from_ref(&condition.member),
            "source condition column",
            false,
        )?;
    }
    if metadata
        .relation_target
        .as_deref()
        .is_some_and(|target| entity == target || target == "Self")
    {
        check(target_columns, "self-target key", true)?;
        if let Some(condition) = &metadata.relation_target_condition {
            check(
                std::slice::from_ref(&condition.member),
                "self-target condition column",
                true,
            )?;
        }
    }
    Ok(())
}

/// Emit independent, cross-crate proofs for every physical target column read
/// by a public relation. Missing metadata and unknown columns fail closed.
pub(crate) fn graphql_relation_target_checks(
    backend: BackendKind,
    entity: &syn::Ident,
    field: &Field,
    metadata: &FieldMetadata,
    target_columns: &[String],
) -> syn::Result<proc_macro2::TokenStream> {
    if !metadata.graphql_readable() {
        return Ok(quote! {});
    }
    let target_name = metadata.relation_target.as_deref().unwrap_or("Unknown");
    let entity_name = entity.to_string();
    let target: syn::Type = syn::parse_str(if target_name == "Self" {
        &entity_name
    } else {
        target_name
    })?;
    let relation = field.ident.as_ref().unwrap().to_string();
    let case_sensitive = backend == BackendKind::Postgres;
    let columns = target_columns.iter().map(|column| (column, "key")).chain(
        metadata
            .relation_target_condition
            .iter()
            .map(|condition| (&condition.member, "condition")),
    );
    let assertions = columns.map(|(column, role)| {
        let identity = relation_column_identity(backend, column, true);
        let message = format!(
            "readable relation `{entity}.{relation}` uses unreadable or unresolved target {role} `{target_name}.{column}`; make the relation unreadable too with #[graphql_orm(read = false)], or make the target column GraphQL-readable (remove private/input_only and set read = true); target metadata must declare the physical column"
        );
        quote! {
            const _: () = assert!(
                ::graphql_orm::graphql::orm::generated_relation_column_is_readable(
                    <#target as ::graphql_orm::graphql::orm::GeneratedRelationReadability>::COLUMNS,
                    #identity,
                    #case_sensitive,
                ),
                concat!(#message),
            );
        }
    });
    Ok(quote! { #(#assertions)* })
}

/// Whether any `#[graphql(...)]` attribute declares `complex`, wherever it
/// appears among the attribute's other options. Malformed `graphql` metadata is
/// an error rather than a silent "not complex".
pub(crate) fn has_graphql_complex(attrs: &[syn::Attribute]) -> syn::Result<bool> {
    let mut complex = false;
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("graphql")) {
        attr.parse_nested_meta(|option| {
            if option.path.is_ident("complex") && !option.input.peek(syn::Token![=]) {
                complex = true;
            }
            skip_foreign_option(&option)
        })?;
    }
    Ok(complex)
}

/// Consume the value of an option this crate does not read from an attribute
/// that another crate owns (async-graphql, serde): `key = expression` or
/// `key(...)`. Leaving it unconsumed would stop `parse_nested_meta` before the
/// options that follow, so their order would decide whether they are seen.
pub(crate) fn skip_foreign_option(option: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<()> {
    if option.input.peek(syn::Token![=]) {
        let _: syn::Expr = option.value()?.parse()?;
    } else if option.input.peek(syn::token::Paren) {
        let content;
        syn::parenthesized!(content in option.input);
        let _: proc_macro2::TokenStream = content.parse()?;
    }
    Ok(())
}

// ============================================================================
// Field Metadata Parsing
// ============================================================================

#[derive(Clone)]
pub(crate) struct FieldMetadata {
    pub(crate) graphql_name: Option<String>,
    pub(crate) serde_name: Option<String>,
    pub(crate) db_column: Option<String>,
    pub(crate) decimal: Option<(u8, u8)>,
    pub(crate) mssql_money: bool,
    pub(crate) description: Option<String>,
    pub(crate) classification: Option<String>,
    pub(crate) non_exportable: bool,
    pub(crate) filterable: Option<String>,
    pub(crate) sortable: bool,
    pub(crate) unique: bool,
    pub(crate) is_primary_key: bool,
    pub(crate) is_version: bool,
    /// `#[graphql_orm(version = "managed")]`: every generated write advances it.
    pub(crate) managed_version: bool,
    pub(crate) minimum: Option<String>,
    pub(crate) maximum: Option<String>,
    pub(crate) exclusive_minimum: Option<String>,
    pub(crate) exclusive_maximum: Option<String>,
    pub(crate) non_negative: bool,
    pub(crate) min_length: Option<usize>,
    pub(crate) max_length: Option<usize>,
    pub(crate) one_of: Vec<String>,
    pub(crate) gte_field: Option<String>,
    pub(crate) gt_field: Option<String>,
    pub(crate) lte_field: Option<String>,
    pub(crate) lt_field: Option<String>,
    pub(crate) is_relation: bool,
    pub(crate) relation_target: Option<String>,
    pub(crate) relation_from: Option<String>,
    pub(crate) relation_to: Option<String>,
    pub(crate) relation_from_fields: Option<Vec<String>>,
    pub(crate) relation_to_fields: Option<Vec<String>>,
    pub(crate) relation_multiple: bool,
    pub(crate) relation_emit_foreign_key: Option<bool>,
    pub(crate) relation_on_delete: Option<String>,
    pub(crate) relation_propagate_change: Option<String>,
    pub(crate) relation_source_condition: Option<RelationConditionMetadata>,
    pub(crate) relation_target_condition: Option<RelationConditionMetadata>,
    pub(crate) relation_filter: bool,
    pub(crate) relation_order_aggregate: Option<RelationOrderAggregateMetadata>,
    pub(crate) skip_db: bool,
    /// Skip from generated public GraphQL Create/Update inputs only; field remains in DB,
    /// the Rust entity, and generated trusted Rust Create/Update input structs.
    pub(crate) skip_input: bool,
    pub(crate) is_private: bool,
    pub(crate) sensitive: bool,
    pub(crate) is_date_field: bool,
    pub(crate) is_boolean_field: bool,
    pub(crate) is_json_field: bool,
    pub(crate) spatial: Option<SpatialFieldMetadata>,
    pub(crate) search: Option<SearchFieldMetadata>,
    pub(crate) search_json: Vec<SearchJsonPathMetadata>,
    pub(crate) search_relation: Option<SearchRelationMetadata>,
    /// Sync read transform: fn(T) -> T
    /// Applied after reading from the database row (e.g., decryption)
    pub(crate) transform_read: Option<String>,
    pub(crate) default: Option<String>,
    pub(crate) suppress_implicit_default: bool,
    pub(crate) host_managed_timestamp: bool,
    pub(crate) auto_generated: Option<bool>,
    pub(crate) backup_policy: Option<String>,
    /// If true, include in Create/Update inputs even if #[graphql(skip)] is set.
    /// Useful for fields that should be writable but never exposed in queries.
    pub(crate) input_only: bool,
    pub(crate) graphql_skip: bool,
    pub(crate) subscription_custom_resolver: bool,
    pub(crate) read: bool,
    pub(crate) write: bool,
    pub(crate) filter: bool,
    pub(crate) order: bool,
    pub(crate) subscribe: bool,
    pub(crate) read_policy: Option<String>,
    pub(crate) write_policy: Option<String>,
}

pub(crate) fn decimal_definition_tokens(meta: &FieldMetadata) -> proc_macro2::TokenStream {
    if meta.mssql_money {
        quote! { ::graphql_orm::graphql::orm::DecimalDef::mssql_money() }
    } else {
        let (precision, scale) = meta.decimal.expect("validated decimal metadata");
        quote! { ::graphql_orm::graphql::orm::DecimalDef::macro_validated(#precision, #scale) }
    }
}

#[derive(Clone)]
pub(crate) enum RelationConditionValue {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
}

#[derive(Clone)]
pub(crate) struct RelationConditionMetadata {
    pub(crate) member: String,
    pub(crate) value: RelationConditionValue,
}

#[derive(Clone)]
pub(crate) struct RelationOrderAggregateMetadata {
    pub(crate) name: String,
    pub(crate) aggregate: String,
}

#[derive(Clone)]
pub(crate) struct SearchFieldMetadata {
    pub(crate) weight: String,
    pub(crate) alias: Option<String>,
    pub(crate) policy: Option<String>,
}

impl Default for SearchFieldMetadata {
    fn default() -> Self {
        Self {
            weight: "D".to_string(),
            alias: None,
            policy: None,
        }
    }
}

#[derive(Clone)]
pub(crate) struct SearchJsonPathMetadata {
    pub(crate) path: String,
    pub(crate) weight: String,
    pub(crate) policy: Option<String>,
}

impl Default for SearchJsonPathMetadata {
    fn default() -> Self {
        Self {
            path: String::new(),
            weight: "D".to_string(),
            policy: None,
        }
    }
}

#[derive(Clone)]
pub(crate) struct SearchRelationMetadata {
    pub(crate) fields: Vec<String>,
    pub(crate) weight: String,
    pub(crate) max_items: usize,
    pub(crate) policy: Option<String>,
    pub(crate) propagate_change: String,
}

impl Default for SearchRelationMetadata {
    fn default() -> Self {
        Self {
            fields: Vec::new(),
            weight: "D".to_string(),
            max_items: 100,
            policy: None,
            propagate_change: "up".to_string(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct SpatialFieldMetadata {
    pub(crate) kind: String,
    pub(crate) geometry_type: String,
    pub(crate) srid: i32,
    pub(crate) index: bool,
    pub(crate) index_method: String,
}

impl Default for SpatialFieldMetadata {
    fn default() -> Self {
        Self {
            kind: "geometry".to_string(),
            geometry_type: "Geometry".to_string(),
            srid: 4326,
            index: false,
            index_method: "gist".to_string(),
        }
    }
}

impl Default for FieldMetadata {
    fn default() -> Self {
        Self {
            graphql_name: None,
            serde_name: None,
            db_column: None,
            decimal: None,
            mssql_money: false,
            description: None,
            classification: None,
            non_exportable: false,
            filterable: None,
            sortable: false,
            unique: false,
            is_primary_key: false,
            is_version: false,
            managed_version: false,
            minimum: None,
            maximum: None,
            exclusive_minimum: None,
            exclusive_maximum: None,
            non_negative: false,
            min_length: None,
            max_length: None,
            one_of: Vec::new(),
            gte_field: None,
            gt_field: None,
            lte_field: None,
            lt_field: None,
            is_relation: false,
            relation_target: None,
            relation_from: None,
            relation_to: None,
            relation_from_fields: None,
            relation_to_fields: None,
            relation_multiple: false,
            relation_emit_foreign_key: None,
            relation_on_delete: None,
            relation_propagate_change: None,
            relation_source_condition: None,
            relation_target_condition: None,
            relation_filter: false,
            relation_order_aggregate: None,
            skip_db: false,
            skip_input: false,
            is_private: false,
            sensitive: false,
            is_date_field: false,
            is_boolean_field: false,
            is_json_field: false,
            spatial: None,
            search: None,
            search_json: Vec::new(),
            search_relation: None,
            transform_read: None,
            default: None,
            suppress_implicit_default: false,
            host_managed_timestamp: false,
            auto_generated: None,
            backup_policy: None,
            input_only: false,
            graphql_skip: false,
            subscription_custom_resolver: false,
            read: true,
            write: true,
            filter: true,
            order: true,
            subscribe: true,
            read_policy: None,
            write_policy: None,
        }
    }
}

fn parse_relation_columns(input: ParseStream<'_>) -> syn::Result<Vec<String>> {
    let expr: syn::Expr = input.parse()?;
    match expr {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(lit),
            ..
        }) => Ok(vec![lit.value()]),
        syn::Expr::Array(array) => array
            .elems
            .into_iter()
            .map(|expr| match expr {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(lit),
                    ..
                }) => Ok(lit.value()),
                other => Err(syn::Error::new_spanned(
                    other,
                    "relation column arrays must contain string literals",
                )),
            })
            .collect(),
        other => Err(syn::Error::new_spanned(
            other,
            "relation columns must be a string literal or an array of string literals",
        )),
    }
}

fn parse_relation_condition(
    nested: syn::meta::ParseNestedMeta<'_>,
    member_name: &str,
) -> syn::Result<RelationConditionMetadata> {
    let mut member = None;
    let mut condition_value = None;
    nested.parse_nested_meta(|condition| {
        if condition.path.is_ident(member_name) {
            if member.is_some() {
                return Err(condition.error(format!(
                    "duplicate relation condition `{member_name}`"
                )));
            }
            let value = condition.value()?;
            let literal: syn::LitStr = value.parse()?;
            let literal = literal.value();
            if literal.trim().is_empty() {
                return Err(condition.error(format!(
                    "relation condition `{member_name}` must not be empty"
                )));
            }
            member = Some(literal);
        } else if condition.path.is_ident("equals") {
            if condition_value.is_some() {
                return Err(condition.error("duplicate relation condition `equals`"));
            }
            let value = condition.value()?;
            let literal: syn::Lit = value.parse()?;
            condition_value = Some(match literal {
                syn::Lit::Str(value) => RelationConditionValue::String(value.value()),
                syn::Lit::Int(value) => {
                    RelationConditionValue::Integer(value.base10_parse()?)
                }
                syn::Lit::Float(value) => RelationConditionValue::Float(value.base10_parse()?),
                syn::Lit::Bool(value) => RelationConditionValue::Boolean(value.value),
                other => {
                    return Err(syn::Error::new_spanned(
                        other,
                        "relation condition values must be string, integer, float, or boolean literals",
                    ));
                }
            });
        } else {
            return Err(condition.error("unsupported relation condition option"));
        }
        Ok(())
    })?;

    Ok(RelationConditionMetadata {
        member: member.ok_or_else(|| {
            syn::Error::new(
                nested.path.span(),
                format!("relation condition requires `{member_name}`"),
            )
        })?,
        value: condition_value.ok_or_else(|| {
            syn::Error::new(nested.path.span(), "relation condition requires `equals`")
        })?,
    })
}

fn parse_string_array_expr(input: ParseStream<'_>, message: &str) -> syn::Result<Vec<String>> {
    let expr: syn::Expr = input.parse()?;
    match expr {
        syn::Expr::Array(array) => array
            .elems
            .into_iter()
            .map(|expr| match expr {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(lit),
                    ..
                }) => Ok(lit.value()),
                other => Err(syn::Error::new_spanned(other, message)),
            })
            .collect(),
        other => Err(syn::Error::new_spanned(other, message)),
    }
}

fn parse_projection_field_array(input: ParseStream<'_>) -> syn::Result<Vec<String>> {
    let expr: syn::Expr = input.parse()?;
    let syn::Expr::Array(array) = expr else {
        return Err(syn::Error::new_spanned(
            expr,
            "projection fields must be an array of Rust field identifiers",
        ));
    };
    array
        .elems
        .into_iter()
        .map(|expr| match expr {
            syn::Expr::Path(path) if path.path.segments.len() == 1 => {
                Ok(path.path.segments[0].ident.to_string())
            }
            other => Err(syn::Error::new_spanned(
                other,
                "projection fields must contain Rust field identifiers, not strings or paths",
            )),
        })
        .collect()
}

fn validate_search_weight(value: &str, span: proc_macro2::Span) -> syn::Result<()> {
    match value {
        "A" | "B" | "C" | "D" => Ok(()),
        _ => Err(syn::Error::new(
            span,
            "search weight must be one of \"A\", \"B\", \"C\", or \"D\"",
        )),
    }
}

fn validate_search_json_path(value: &str, span: proc_macro2::Span) -> syn::Result<()> {
    if !value.starts_with('$') {
        return Err(syn::Error::new(
            span,
            "search_json path must start with `$`",
        ));
    }

    let mut saw_segment = false;
    let mut index = 1;
    while index < value.len() {
        let remainder = &value[index..];
        if remainder.starts_with('.') {
            index += 1;
            let field_start = index;
            while index < value.len() {
                let ch = value[index..].chars().next().unwrap_or_default();
                if ch == '.' || ch == '[' {
                    break;
                }
                if !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-') {
                    return Err(syn::Error::new(
                        span,
                        "unsupported search_json path field character; field names may contain ASCII letters, digits, underscores, and hyphens",
                    ));
                }
                index += ch.len_utf8();
            }
            if field_start == index {
                return Err(syn::Error::new(
                    span,
                    "search_json path field segments cannot be empty",
                ));
            }
            saw_segment = true;
        } else if remainder.starts_with("[*]") {
            index += 3;
            saw_segment = true;
        } else {
            return Err(syn::Error::new(
                span,
                "unsupported search_json path syntax; supported forms include $.field, $.nested.field, $.array[*].field, and $[*].field",
            ));
        }
    }

    if !saw_segment {
        return Err(syn::Error::new(
            span,
            "search_json path must select at least one field or wildcard",
        ));
    }

    Ok(())
}

fn search_weight_tokens(
    value: &str,
    span: proc_macro2::Span,
) -> syn::Result<proc_macro2::TokenStream> {
    validate_search_weight(value, span)?;
    Ok(match value {
        "A" => quote! { ::graphql_orm::graphql::orm::SearchWeight::A },
        "B" => quote! { ::graphql_orm::graphql::orm::SearchWeight::B },
        "C" => quote! { ::graphql_orm::graphql::orm::SearchWeight::C },
        "D" => quote! { ::graphql_orm::graphql::orm::SearchWeight::D },
        _ => unreachable!(),
    })
}

fn validate_spatial_geometry_type(value: &str, span: proc_macro2::Span) -> syn::Result<()> {
    match value {
        "Geometry" | "Point" | "LineString" | "Polygon" | "MultiPoint" | "MultiLineString"
        | "MultiPolygon" | "GeometryCollection" => Ok(()),
        _ => Err(syn::Error::new(
            span,
            "unsupported spatial geometry_type; expected Geometry, Point, LineString, Polygon, MultiPoint, MultiLineString, MultiPolygon, or GeometryCollection",
        )),
    }
}

pub(crate) fn validate_semantic_description(
    value: &str,
    span: proc_macro2::Span,
) -> syn::Result<()> {
    let invalid_character = |character: char| {
        character.is_control()
            || matches!(
                character,
                '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}'
            )
    };
    if value.trim().is_empty()
        || value.trim() != value
        || value.len() > 1024
        || value.chars().any(invalid_character)
    {
        return Err(syn::Error::new(
            span,
            "semantic descriptions must be trimmed, non-empty, at most 1024 bytes, and contain no control or directional characters",
        ));
    }
    Ok(())
}

pub(crate) fn validate_semantic_classification(
    value: &str,
    span: proc_macro2::Span,
) -> syn::Result<()> {
    match value {
        "public" | "internal" | "confidential" | "restricted" | "secret" => Ok(()),
        _ => Err(syn::Error::new(
            span,
            "semantic classification must be public, internal, confidential, restricted, or secret",
        )),
    }
}

pub(crate) fn semantic_classification_rank(value: &str) -> u8 {
    match value {
        "public" => 0,
        "internal" => 1,
        "confidential" => 2,
        "restricted" => 3,
        "secret" => 4,
        _ => unreachable!("classification is validated before ranking"),
    }
}

pub(crate) fn semantic_doc_description(attrs: &[syn::Attribute]) -> syn::Result<Option<String>> {
    let mut lines: Vec<String> = Vec::new();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("doc")) {
        let syn::Meta::NameValue(value) = &attr.meta else {
            continue;
        };
        let syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(value),
            ..
        }) = &value.value
        else {
            return Err(syn::Error::new_spanned(
                attr,
                "documentation must be a string",
            ));
        };
        let line = value.value();
        let line = line.trim();
        if !line.is_empty() {
            lines.push(line.to_owned());
        }
    }
    if lines.is_empty() {
        return Ok(None);
    }
    let description = lines.join(" ");
    validate_semantic_description(&description, proc_macro2::Span::call_site())?;
    Ok(Some(description))
}

pub(crate) fn humanize_identifier(value: &str) -> String {
    let words = value.to_case(Case::Title);
    if words.is_empty() {
        "GraphQL value".to_owned()
    } else {
        words
    }
}

pub(crate) fn semantic_classification_tokens(
    value: &str,
    span: proc_macro2::Span,
) -> syn::Result<proc_macro2::TokenStream> {
    validate_semantic_classification(value, span)?;
    Ok(match value {
        "public" => quote! { ::graphql_orm::graphql::orm::GraphqlSemanticClassification::Public },
        "internal" => {
            quote! { ::graphql_orm::graphql::orm::GraphqlSemanticClassification::Internal }
        }
        "confidential" => {
            quote! { ::graphql_orm::graphql::orm::GraphqlSemanticClassification::Confidential }
        }
        "restricted" => {
            quote! { ::graphql_orm::graphql::orm::GraphqlSemanticClassification::Restricted }
        }
        "secret" => quote! { ::graphql_orm::graphql::orm::GraphqlSemanticClassification::Secret },
        _ => unreachable!(),
    })
}

pub(crate) fn spatial_geometry_type_tokens(
    value: &str,
    span: proc_macro2::Span,
) -> syn::Result<proc_macro2::TokenStream> {
    validate_spatial_geometry_type(value, span)?;
    Ok(match value {
        "Geometry" => quote! { ::graphql_orm::graphql::orm::SpatialGeometryType::Geometry },
        "Point" => quote! { ::graphql_orm::graphql::orm::SpatialGeometryType::Point },
        "LineString" => quote! { ::graphql_orm::graphql::orm::SpatialGeometryType::LineString },
        "Polygon" => quote! { ::graphql_orm::graphql::orm::SpatialGeometryType::Polygon },
        "MultiPoint" => quote! { ::graphql_orm::graphql::orm::SpatialGeometryType::MultiPoint },
        "MultiLineString" => {
            quote! { ::graphql_orm::graphql::orm::SpatialGeometryType::MultiLineString }
        }
        "MultiPolygon" => {
            quote! { ::graphql_orm::graphql::orm::SpatialGeometryType::MultiPolygon }
        }
        "GeometryCollection" => {
            quote! { ::graphql_orm::graphql::orm::SpatialGeometryType::GeometryCollection }
        }
        _ => unreachable!(),
    })
}

#[derive(Clone)]
pub(crate) struct ParsedField {
    pub(crate) field: Field,
    pub(crate) meta: FieldMetadata,
}

pub(crate) fn parse_field_metadata(field: &Field) -> syn::Result<FieldMetadata> {
    let mut meta = FieldMetadata::default();

    for attr in &field.attrs {
        if let Some(ident) = attr.path().get_ident() {
            match ident.to_string().as_str() {
                "graphql" => {
                    if let Ok(options) = attr.parse_args_with(
                        syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
                    ) {
                        meta.subscription_custom_resolver |= options.iter().any(|option| {
                            ["guard", "visible", "skip_output", "flatten", "derived"]
                                .iter()
                                .any(|name| option.path().is_ident(name))
                        });
                        // Read `name` wherever it appears; the nested parse
                        // below stops at the first option it does not consume.
                        for option in &options {
                            if let syn::Meta::NameValue(value) = option
                                && value.path.is_ident("name")
                            {
                                meta.graphql_name = Some(crate::composed_identity::string_literal(
                                    &value.value,
                                    "name",
                                )?);
                            }
                        }
                    }
                    attr.parse_nested_meta(|nested| {
                        if nested.path.is_ident("name") {
                            let value = nested.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            meta.graphql_name = Some(lit.value());
                        } else if nested.path.is_ident("skip") {
                            // Any `skip` keeps the legacy ORM input exclusion, even
                            // `skip = false`; `has_graphql_object_skip` decides the
                            // object getter. Consume the value so later options parse.
                            if nested.input.peek(syn::Token![=]) {
                                nested.value()?.parse::<syn::LitBool>()?;
                            }
                            meta.skip_input = true;
                            meta.graphql_skip = true;
                        } else {
                            skip_foreign_option(&nested)?;
                        }
                        Ok(())
                    })?;
                }
                "graphql_orm" => {
                    attr.parse_nested_meta(|nested| {
                        if nested.path.is_ident("private") {
                            meta.is_private = true;
                            meta.read = false;
                            meta.filter = false;
                            meta.order = false;
                            meta.subscribe = false;
                            meta.skip_input = true;
                        } else if nested.path.is_ident("sensitive") {
                            meta.sensitive = true;
                        } else if nested.path.is_ident("description") {
                            let value = nested.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            validate_semantic_description(&lit.value(), lit.span())?;
                            meta.description = Some(lit.value());
                        } else if nested.path.is_ident("classification") {
                            let value = nested.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            validate_semantic_classification(&lit.value(), lit.span())?;
                            meta.classification = Some(lit.value());
                        } else if nested.path.is_ident("non_exportable") {
                            meta.non_exportable = true;
                        } else if nested.path.is_ident("version") {
                            if nested.input.peek(syn::Token![=]) {
                                let value = nested.value()?;
                                let lit: syn::LitStr = value.parse()?;
                                match lit.value().as_str() {
                                    "managed" => meta.managed_version = true,
                                    "cas" => {}
                                    _ => {
                                        return Err(syn::Error::new(
                                            lit.span(),
                                            "version accepts \"managed\" or \"cas\"; bare `version` is \"cas\"",
                                        ));
                                    }
                                }
                            }
                            meta.is_version = true;
                            meta.skip_input = true;
                            meta.write = false;
                        } else if nested.path.is_ident("min") {
                            let value = nested.value()?;
                            let lit: syn::Lit = value.parse()?;
                            meta.minimum = Some(match lit {
                                syn::Lit::Int(value) => value.base10_digits().to_string(),
                                syn::Lit::Float(value) => value.base10_digits().to_string(),
                                _ => return Err(syn::Error::new(lit.span(), "min must be numeric")),
                            });
                        } else if nested.path.is_ident("max") {
                            let value = nested.value()?;
                            let lit: syn::Lit = value.parse()?;
                            meta.maximum = Some(match lit {
                                syn::Lit::Int(value) => value.base10_digits().to_string(),
                                syn::Lit::Float(value) => value.base10_digits().to_string(),
                                _ => return Err(syn::Error::new(lit.span(), "max must be numeric")),
                            });
                        } else if nested.path.is_ident("min_exclusive") {
                            let value = nested.value()?;
                            let lit: syn::Lit = value.parse()?;
                            meta.exclusive_minimum = Some(match lit {
                                syn::Lit::Int(value) => value.base10_digits().to_string(),
                                syn::Lit::Float(value) => value.base10_digits().to_string(),
                                _ => {
                                    return Err(syn::Error::new(
                                        lit.span(),
                                        "min_exclusive must be numeric",
                                    ));
                                }
                            });
                        } else if nested.path.is_ident("max_exclusive") {
                            let value = nested.value()?;
                            let lit: syn::Lit = value.parse()?;
                            meta.exclusive_maximum = Some(match lit {
                                syn::Lit::Int(value) => value.base10_digits().to_string(),
                                syn::Lit::Float(value) => value.base10_digits().to_string(),
                                _ => {
                                    return Err(syn::Error::new(
                                        lit.span(),
                                        "max_exclusive must be numeric",
                                    ));
                                }
                            });
                        } else if nested.path.is_ident("non_negative") {
                            meta.non_negative = true;
                        } else if nested.path.is_ident("min_length") {
                            let value = nested.value()?;
                            let lit: syn::LitInt = value.parse()?;
                            meta.min_length = Some(lit.base10_parse()?);
                        } else if nested.path.is_ident("max_length") {
                            let value = nested.value()?;
                            let lit: syn::LitInt = value.parse()?;
                            meta.max_length = Some(lit.base10_parse()?);
                        } else if nested.path.is_ident("one_of") {
                            let value = nested.value()?;
                            meta.one_of = parse_string_array_expr(
                                value,
                                "one_of must be an array of string literals",
                            )?;
                        } else if nested.path.is_ident("gte_field") {
                            let value = nested.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            meta.gte_field = Some(lit.value());
                        } else if nested.path.is_ident("gt_field") {
                            let value = nested.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            meta.gt_field = Some(lit.value());
                        } else if nested.path.is_ident("lte_field") {
                            let value = nested.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            meta.lte_field = Some(lit.value());
                        } else if nested.path.is_ident("lt_field") {
                            let value = nested.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            meta.lt_field = Some(lit.value());
                        } else if nested.path.is_ident("skip_input") {
                            meta.skip_input = true;
                        } else if nested.path.is_ident("json") {
                            meta.is_json_field = true;
                            meta.filter = false;
                            meta.order = false;
                        } else if nested.path.is_ident("read") {
                            let value = nested.value()?;
                            let lit: syn::LitBool = value.parse()?;
                            meta.read = lit.value;
                        } else if nested.path.is_ident("write") {
                            let value = nested.value()?;
                            let lit: syn::LitBool = value.parse()?;
                            meta.write = lit.value;
                            if !meta.write {
                                meta.skip_input = true;
                            }
                        } else if nested.path.is_ident("filter") {
                            let value = nested.value()?;
                            let lit: syn::LitBool = value.parse()?;
                            meta.filter = lit.value;
                        } else if nested.path.is_ident("order") {
                            let value = nested.value()?;
                            let lit: syn::LitBool = value.parse()?;
                            meta.order = lit.value;
                        } else if nested.path.is_ident("subscribe") {
                            let value = nested.value()?;
                            let lit: syn::LitBool = value.parse()?;
                            meta.subscribe = lit.value;
                        } else if nested.path.is_ident("read_policy") {
                            let value = nested.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            meta.read_policy = Some(lit.value());
                        } else if nested.path.is_ident("write_policy") {
                            let value = nested.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            meta.write_policy = Some(lit.value());
                        } else if nested.path.is_ident("db_column") {
                            let value = nested.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            meta.db_column = Some(lit.value());
                        } else if nested.path.is_ident("mssql_money") {
                            if meta.mssql_money {
                                return Err(syn::Error::new(nested.path.span(), "mssql_money may only be declared once per field"));
                            }
                            meta.mssql_money = true;
                        } else if nested.path.is_ident("decimal") {
                            if meta.decimal.is_some() {
                                return Err(syn::Error::new(
                                    nested.path.span(),
                                    "decimal may only be declared once per field",
                                ));
                            }
                            let mut precision = None;
                            let mut scale = None;
                            nested.parse_nested_meta(|decimal_meta| {
                                if decimal_meta.path.is_ident("precision") {
                                    let value = decimal_meta.value()?;
                                    let lit: syn::LitInt = value.parse()?;
                                    precision = Some(lit.base10_parse::<u8>()?);
                                } else if decimal_meta.path.is_ident("scale") {
                                    let value = decimal_meta.value()?;
                                    let lit: syn::LitInt = value.parse()?;
                                    scale = Some(lit.base10_parse::<u8>()?);
                                } else {
                                    return Err(syn::Error::new(
                                        decimal_meta.path.span(),
                                        "unsupported decimal option; expected precision or scale",
                                    ));
                                }
                                Ok(())
                            })?;
                            let precision = precision.ok_or_else(|| {
                                syn::Error::new(nested.path.span(), "decimal requires precision")
                            })?;
                            let scale = scale.ok_or_else(|| {
                                syn::Error::new(nested.path.span(), "decimal requires scale")
                            })?;
                            if precision == 0 || precision > 18 || scale > precision {
                                return Err(syn::Error::new(
                                    nested.path.span(),
                                    "decimal requires 1 <= precision <= 18 and scale <= precision",
                                ));
                            }
                            meta.decimal = Some((precision, scale));
                        } else if nested.path.is_ident("default") {
                            if meta.default.is_some() || meta.suppress_implicit_default {
                                return Err(syn::Error::new(
                                    nested.path.span(),
                                    "default may only be declared once per field",
                                ));
                            }
                            let value = nested.value()?;
                            let lit: syn::Lit = value.parse()?;
                            match lit {
                                syn::Lit::Str(value) => meta.default = Some(value.value()),
                                syn::Lit::Bool(value) if !value.value => {
                                    meta.suppress_implicit_default = true;
                                }
                                _ => {
                                    return Err(syn::Error::new(
                                        lit.span(),
                                        "default must be a SQL string literal or false",
                                    ));
                                }
                            }
                        } else if nested.path.is_ident("timestamp") {
                            if meta.host_managed_timestamp {
                                return Err(nested.error("timestamp may only be declared once per field"));
                            }
                            let value: syn::LitStr = nested.value()?.parse()?;
                            if value.value() != "host" {
                                return Err(syn::Error::new(value.span(), "timestamp must be \"host\""));
                            }
                            meta.host_managed_timestamp = true;
                        } else if nested.path.is_ident("auto_generated") {
                            let value = nested.value()?;
                            let lit: syn::LitBool = value.parse()?;
                            meta.auto_generated = Some(lit.value);
                        } else if nested.path.is_ident("searchable") {
                            let mut search = SearchFieldMetadata::default();
                            nested.parse_nested_meta(|search_meta| {
                                if search_meta.path.is_ident("weight") {
                                    let value = search_meta.value()?;
                                    let lit: syn::LitStr = value.parse()?;
                                    let weight = lit.value();
                                    validate_search_weight(&weight, lit.span())?;
                                    search.weight = weight;
                                } else if search_meta.path.is_ident("alias") {
                                    let value = search_meta.value()?;
                                    let lit: syn::LitStr = value.parse()?;
                                    search.alias = Some(lit.value());
                                } else if search_meta.path.is_ident("policy") {
                                    let value = search_meta.value()?;
                                    let lit: syn::LitStr = value.parse()?;
                                    search.policy = Some(lit.value());
                                } else {
                                    return Err(syn::Error::new(
                                        search_meta.path.span(),
                                        "unsupported searchable option; expected weight, alias, or policy",
                                    ));
                                }
                                Ok(())
                            })?;
                            meta.search = Some(search);
                        } else if nested.path.is_ident("search_json") {
                            let mut search_json = SearchJsonPathMetadata::default();
                            nested.parse_nested_meta(|search_meta| {
                                if search_meta.path.is_ident("path") {
                                    let value = search_meta.value()?;
                                    let lit: syn::LitStr = value.parse()?;
                                    let path = lit.value();
                                    validate_search_json_path(&path, lit.span())?;
                                    search_json.path = path;
                                } else if search_meta.path.is_ident("weight") {
                                    let value = search_meta.value()?;
                                    let lit: syn::LitStr = value.parse()?;
                                    let weight = lit.value();
                                    validate_search_weight(&weight, lit.span())?;
                                    search_json.weight = weight;
                                } else if search_meta.path.is_ident("policy") {
                                    let value = search_meta.value()?;
                                    let lit: syn::LitStr = value.parse()?;
                                    search_json.policy = Some(lit.value());
                                } else {
                                    return Err(syn::Error::new(
                                        search_meta.path.span(),
                                        "unsupported search_json option; expected path, weight, or policy",
                                    ));
                                }
                                Ok(())
                            })?;
                            if search_json.path.is_empty() {
                                return Err(syn::Error::new(
                                    nested.path.span(),
                                    "search_json requires path = \"...\"",
                                ));
                            }
                            meta.search_json.push(search_json);
                        } else if nested.path.is_ident("search_relation") {
                            let mut search_relation = SearchRelationMetadata::default();
                            nested.parse_nested_meta(|search_meta| {
                                if search_meta.path.is_ident("fields") {
                                    let value = search_meta.value()?;
                                    search_relation.fields = parse_string_array_expr(
                                        value,
                                        "search_relation fields must be an array of string literals",
                                    )?;
                                } else if search_meta.path.is_ident("weight") {
                                    let value = search_meta.value()?;
                                    let lit: syn::LitStr = value.parse()?;
                                    let weight = lit.value();
                                    validate_search_weight(&weight, lit.span())?;
                                    search_relation.weight = weight;
                                } else if search_meta.path.is_ident("max_items") {
                                    let value = search_meta.value()?;
                                    let lit: syn::LitInt = value.parse()?;
                                    search_relation.max_items = lit.base10_parse()?;
                                } else if search_meta.path.is_ident("policy") {
                                    let value = search_meta.value()?;
                                    let lit: syn::LitStr = value.parse()?;
                                    search_relation.policy = Some(lit.value());
                                } else if search_meta.path.is_ident("propagate_change") {
                                    let value = search_meta.value()?;
                                    let lit: syn::LitStr = value.parse()?;
                                    let value = lit.value();
                                    if value != "up" {
                                        return Err(syn::Error::new(
                                            lit.span(),
                                            "search_relation propagate_change only supports \"up\"",
                                        ));
                                    }
                                    search_relation.propagate_change = value;
                                } else {
                                    return Err(syn::Error::new(
                                        search_meta.path.span(),
                                        "unsupported search_relation option; expected fields, weight, max_items, policy, or propagate_change",
                                    ));
                                }
                                Ok(())
                            })?;
                            if search_relation.fields.is_empty() {
                                return Err(syn::Error::new(
                                    nested.path.span(),
                                    "search_relation requires fields = [..]",
                                ));
                            }
                            meta.search_relation = Some(search_relation);
                        } else if nested.path.is_ident("spatial") {
                            let mut spatial = SpatialFieldMetadata::default();
                            nested.parse_nested_meta(|spatial_meta| {
                                if spatial_meta.path.is_ident("kind") {
                                    let value = spatial_meta.value()?;
                                    let lit: syn::LitStr = value.parse()?;
                                    let kind = lit.value();
                                    if kind != "geometry" {
                                        return Err(syn::Error::new(
                                            lit.span(),
                                            "unsupported spatial kind; only kind = \"geometry\" is supported",
                                        ));
                                    }
                                    spatial.kind = kind;
                                } else if spatial_meta.path.is_ident("geometry_type") {
                                    let value = spatial_meta.value()?;
                                    let lit: syn::LitStr = value.parse()?;
                                    let geometry_type = lit.value();
                                    validate_spatial_geometry_type(&geometry_type, lit.span())?;
                                    spatial.geometry_type = geometry_type;
                                } else if spatial_meta.path.is_ident("srid") {
                                    let value = spatial_meta.value()?;
                                    let lit: syn::LitInt = value.parse()?;
                                    spatial.srid = lit.base10_parse()?;
                                } else if spatial_meta.path.is_ident("index") {
                                    let value = spatial_meta.value()?;
                                    let lit: syn::LitBool = value.parse()?;
                                    spatial.index = lit.value;
                                } else if spatial_meta.path.is_ident("index_method") {
                                    let value = spatial_meta.value()?;
                                    let lit: syn::LitStr = value.parse()?;
                                    let method = lit.value();
                                    if method != "gist" {
                                        return Err(syn::Error::new(
                                            lit.span(),
                                            "unsupported spatial index_method; only index_method = \"gist\" is supported",
                                        ));
                                    }
                                    spatial.index_method = method;
                                }
                                Ok(())
                            })?;
                            meta.spatial = Some(spatial);
                            meta.is_json_field = true;
                            meta.order = false;
                        }
                        Ok(())
                    })?;
                }
                "backup" => {
                    attr.parse_nested_meta(|nested| {
                        let policy = ["include", "exclude", "redact"]
                            .into_iter()
                            .find(|policy| nested.path.is_ident(policy))
                            .ok_or_else(|| {
                                nested.error(
                                    "unsupported backup option; expected include, exclude, or redact",
                                )
                            })?;
                        if meta.backup_policy.replace(policy.to_string()).is_some() {
                            return Err(nested.error("a field takes one backup policy"));
                        }
                        Ok(())
                    })?;
                }
                "serde" => {
                    attr.parse_nested_meta(|nested| {
                        if nested.path.is_ident("rename") && nested.input.peek(syn::Token![=]) {
                            let value = nested.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            meta.serde_name = Some(lit.value());
                            Ok(())
                        } else {
                            skip_foreign_option(&nested)
                        }
                    })?;
                }
                "primary_key" => {
                    meta.is_primary_key = true;
                }
                "filterable" => {
                    if let Meta::List(_) = &attr.meta {
                        attr.parse_nested_meta(|nested| {
                            if nested.path.is_ident("type") {
                                let value = nested.value()?;
                                let lit: syn::LitStr = value.parse()?;
                                meta.filterable = Some(lit.value());
                                Ok(())
                            } else {
                                Err(nested.error("unsupported filterable option; expected type"))
                            }
                        })?;
                    } else {
                        meta.filterable = Some("string".to_string());
                    }
                }
                "sortable" => {
                    meta.sortable = true;
                }
                "unique" => {
                    meta.unique = true;
                }
                "db_column" => {
                    if let Meta::NameValue(nv) = &attr.meta {
                        if let syn::Expr::Lit(syn::ExprLit {
                            lit: syn::Lit::Str(lit),
                            ..
                        }) = &nv.value
                        {
                            meta.db_column = Some(lit.value());
                        }
                    }
                }
                "relation" => {
                    meta.is_relation = true;
                    attr.parse_nested_meta(|nested| {
                        if nested.path.is_ident("target") {
                            let value = nested.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            meta.relation_target = Some(lit.value());
                        } else if nested.path.is_ident("from") {
                            let value = nested.value()?;
                            let columns = parse_relation_columns(value)?;
                            if let Some(column) = columns.first() {
                                meta.relation_from = Some(column.clone());
                            }
                            meta.relation_from_fields = Some(columns);
                        } else if nested.path.is_ident("to") {
                            let value = nested.value()?;
                            let columns = parse_relation_columns(value)?;
                            if let Some(column) = columns.first() {
                                meta.relation_to = Some(column.clone());
                            }
                            meta.relation_to_fields = Some(columns);
                        } else if nested.path.is_ident("on_delete") {
                            let value = nested.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            meta.relation_on_delete = Some(lit.value());
                        } else if nested.path.is_ident("emit_fk") {
                            let value = nested.value()?;
                            let lit: syn::LitBool = value.parse()?;
                            meta.relation_emit_foreign_key = Some(lit.value);
                        } else if nested.path.is_ident("propagate_change") {
                            let value = nested.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            meta.relation_propagate_change = Some(lit.value());
                        } else if nested.path.is_ident("source_condition") {
                            if meta.relation_source_condition.is_some() {
                                return Err(nested.error("duplicate relation source condition"));
                            }
                            meta.relation_source_condition =
                                Some(parse_relation_condition(nested, "field")?);
                        } else if nested.path.is_ident("target_condition") {
                            if meta.relation_target_condition.is_some() {
                                return Err(nested.error("duplicate relation target condition"));
                            }
                            meta.relation_target_condition =
                                Some(parse_relation_condition(nested, "column")?);
                        } else if nested.path.is_ident("filter") {
                            meta.relation_filter = true;
                        } else if nested.path.is_ident("order_aggregate") {
                            if meta.relation_order_aggregate.is_some() {
                                return Err(nested.error("duplicate relation order_aggregate"));
                            }
                            let mut name = None;
                            let mut aggregate = None;
                            nested.parse_nested_meta(|option| {
                                if option.path.is_ident("name") {
                                    let value = option.value()?;
                                    let literal: syn::LitStr = value.parse()?;
                                    validate_graphql_order_name(&literal.value(), literal.span())?;
                                    name = Some(literal.value());
                                } else if option.path.is_ident("aggregate") {
                                    let value = option.value()?;
                                    let literal: syn::LitStr = value.parse()?;
                                    let value = literal.value();
                                    if value != "count" {
                                        return Err(syn::Error::new(
                                            literal.span(),
                                            "relation order_aggregate currently supports only aggregate = \"count\"",
                                        ));
                                    }
                                    aggregate = Some(value);
                                } else {
                                    return Err(option.error(
                                        "unsupported relation order_aggregate option; expected name or aggregate",
                                    ));
                                }
                                Ok(())
                            })?;
                            meta.relation_order_aggregate = Some(RelationOrderAggregateMetadata {
                                name: name.ok_or_else(|| {
                                    syn::Error::new(
                                        nested.path.span(),
                                        "relation order_aggregate requires name = \"GraphQLField\"",
                                    )
                                })?,
                                aggregate: aggregate.ok_or_else(|| {
                                    syn::Error::new(
                                        nested.path.span(),
                                        "relation order_aggregate requires aggregate = \"count\"",
                                    )
                                })?,
                            });
                        } else if nested.path.is_ident("multiple") {
                            meta.relation_multiple = true;
                        } else {
                            return Err(nested.error("unsupported relation option"));
                        }
                        Ok(())
                    })?;
                }
                "skip_db" => {
                    meta.skip_db = true;
                }
                "date_field" => {
                    meta.is_date_field = true;
                }
                "boolean_field" => {
                    meta.is_boolean_field = true;
                }
                "json_field" => {
                    meta.is_json_field = true;
                    meta.filter = false;
                    meta.order = false;
                }
                "transform" => {
                    attr.parse_nested_meta(|nested| {
                        if nested.path.is_ident("write") {
                            let name = field.ident.as_ref().map(ToString::to_string).unwrap_or_default();
                            return Err(syn::Error::new_spanned(
                                &nested.path,
                                format!("field `{name}`: #[transform(write = ...)] is unsupported with generated entity APIs; use a WriteInputTransform on Database instead. Existing rows were stored untransformed; #[transform(read = ...)] is still supported"),
                            ));
                        } else if nested.path.is_ident("read") {
                            let value = nested.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            meta.transform_read = Some(lit.value());
                        } else {
                            return Err(nested
                                .error("unsupported transform option; expected write or read"));
                        }
                        Ok(())
                    })?;
                }
                "input_only" => {
                    meta.input_only = true;
                }
                _ => {}
            }
        }
    }

    if meta.description.is_none() {
        meta.description = semantic_doc_description(&field.attrs)?.or_else(|| {
            field
                .ident
                .as_ref()
                .map(|ident| humanize_identifier(&ident.to_string()))
        });
    }
    if meta.sensitive {
        if meta
            .classification
            .as_deref()
            .is_some_and(|value| value != "secret")
        {
            return Err(syn::Error::new_spanned(
                field,
                "sensitive fields cannot declare a classification below secret",
            ));
        }
        meta.classification = Some("secret".to_owned());
        meta.non_exportable = true;
    }

    let field_type = option_inner_type(&field.ty).unwrap_or(&field.ty);
    let is_decimal = type_path_last_ident(field_type).is_some_and(|ident| ident == "Decimal");
    if meta.mssql_money {
        if !is_decimal {
            return Err(syn::Error::new_spanned(
                field,
                "mssql_money requires rust_decimal::Decimal or Option<rust_decimal::Decimal>",
            ));
        }
        if meta.decimal.is_some() {
            return Err(syn::Error::new_spanned(
                field,
                "mssql_money cannot be combined with decimal metadata",
            ));
        }
        meta.decimal = Some((19, 4));
    }
    if is_decimal && meta.decimal.is_none() {
        return Err(syn::Error::new_spanned(
            field,
            "rust_decimal::Decimal fields require #[graphql_orm(decimal(precision = P, scale = S))]",
        ));
    }
    if !is_decimal && meta.decimal.is_some() {
        return Err(syn::Error::new_spanned(
            field,
            "decimal metadata is only valid on rust_decimal::Decimal fields",
        ));
    }

    Ok(meta)
}

/// Host-only contracts use the same input eligibility and Rust input types as
/// the generated resolvers. They do not create GraphQL fields or descriptions.
fn generated_hidden_input_members(
    struct_name: &syn::Ident,
    fields: &syn::punctuated::Punctuated<Field, syn::token::Comma>,
    entity_meta: &EntityMetadata,
    entity_classification: &str,
    graphql_rename_fields: Option<&str>,
    serde_rename_all: Option<&str>,
) -> syn::Result<Vec<proc_macro2::TokenStream>> {
    let keys = crate::operations::collect_primary_key_fields(
        struct_name,
        fields,
        graphql_rename_fields,
        serde_rename_all,
    )
    .unwrap_or_default();
    let auto_generated_pk = !(keys.len() > 1 && entity_meta.append_only)
        && keys
            .first()
            .is_some_and(|key| key.meta.auto_generated.unwrap_or(key.rust_name == "id"));
    let mut members = Vec::new();
    for field in fields {
        let meta = parse_field_metadata(field)?;
        if meta.is_relation || meta.skip_db {
            continue;
        }
        let hidden = !meta.graphql_readable()
            || meta.non_exportable
            || meta
                .classification
                .as_deref()
                .unwrap_or(entity_classification)
                == "secret";
        if !hidden {
            continue;
        }
        let rust_name = field.ident.as_ref().expect("named fields").to_string();
        let name = graphql_field_name(&meta, &rust_name, graphql_rename_fields, serde_rename_all);
        let mut append = |kind: &str, input: String, ty: proc_macro2::TokenStream| {
            let kind = syn::Ident::new(kind, field.span());
            members.push(quote! {
                ::graphql_orm::graphql::orm::GraphqlGeneratedInputMember {
                    kind: ::graphql_orm::graphql::orm::GraphqlGeneratedInputKind::#kind,
                    input_type: ::std::string::String::from(#input),
                    field_name: ::std::string::String::from(#name),
                    type_ref: ::graphql_orm::graphql::orm::parse_graphql_type(
                        &<#ty as ::graphql_orm::async_graphql::InputType>::qualified_type_name(),
                    ).expect("generated input member has a valid GraphQL type"),
                    required: <#ty as ::graphql_orm::async_graphql::InputType>::qualified_type_name().ends_with('!'),
                }
            });
        };
        let write_type = graphql_write_input_type(&field.ty, &meta);
        if crate::operations::create_input_visibility(&meta, &rust_name, auto_generated_pk) {
            append(
                "Create",
                format!("Create{struct_name}Input"),
                quote! { #write_type },
            );
        }
        if !meta.is_primary_key
            && !meta.timestamp_behavior(&rust_name).exclude_input
            && meta.write
            && (!meta.skip_input || meta.input_only)
        {
            append(
                "Update",
                format!("Update{struct_name}Input"),
                quote! { Option<#write_type> },
            );
        }
        if meta.filter
            && let Some(kind) = meta.filterable.as_deref()
        {
            let helper = match kind {
                "string" => "StringFilter",
                "uuid" => "UuidFilter",
                "number" => "IntFilter",
                "int64" => "Int64Filter",
                "decimal" => "DecimalFilter",
                "boolean" | "bool" => "BoolFilter",
                "bytes" => "BytesFilter",
                "date" => "DateFilter",
                "spatial" => "SpatialFilter",
                _ => {
                    return Err(syn::Error::new_spanned(
                        field,
                        "unsupported canonical filter type",
                    ));
                }
            };
            let helper = syn::Ident::new(helper, field.span());
            append(
                "Where",
                format!("{struct_name}WhereInput"),
                quote! { Option<::graphql_orm::graphql::filters::#helper> },
            );
        }
        if meta.sortable && meta.order {
            append(
                "OrderBy",
                format!("{struct_name}OrderByInput"),
                quote! { Option<::graphql_orm::graphql::orm::OrderDirection> },
            );
        }
        let is_key = keys.iter().any(|key| key.rust_name == rust_name);
        if keys.len() > 1 && is_key {
            let ty = &field.ty;
            append("Key", format!("{struct_name}KeyInput"), quote! { #ty });
        }
        if entity_meta.keyset.is_some()
            && meta.graphql_readable()
            && !meta.graphql_skip
            && meta.transform_read.is_none()
            && (is_key || (meta.sortable && meta.order))
            && crate::operations::runtime_keyset_value(&field.ty).is_some()
        {
            append(
                "KeysetOrderBy",
                format!("{struct_name}KeysetOrderByInput"),
                quote! { Option<::graphql_orm::graphql::orm::OrderDirection> },
            );
        }
    }
    Ok(members)
}

pub(crate) fn graphql_write_input_type(field_type: &syn::Type, meta: &FieldMetadata) -> syn::Type {
    if meta.is_json_field || meta.spatial.is_some() {
        if let Some(inner) = option_inner_type(field_type) {
            syn::parse_quote! { Option<::graphql_orm::async_graphql::Json<#inner>> }
        } else {
            syn::parse_quote! { ::graphql_orm::async_graphql::Json<#field_type> }
        }
    } else {
        field_type.clone()
    }
}

/// Keep legacy Rust-name input exclusion distinct from physical-column updates.
/// Host management disables both; it never changes column/default metadata.
#[derive(Clone, Copy)]
pub(crate) struct TimestampBehavior {
    pub(crate) exclude_input: bool,
    pub(crate) update_updated_at: bool,
}

impl FieldMetadata {
    /// Eligibility for automatic GraphQL reads, independent of explicit scalar
    /// filter/sort capabilities and additional surface-specific restrictions.
    pub(crate) fn graphql_readable(&self) -> bool {
        self.read && !self.is_private && !self.input_only
    }

    pub(crate) fn timestamp_behavior(&self, rust_name: &str) -> TimestampBehavior {
        TimestampBehavior {
            exclude_input: !self.host_managed_timestamp
                && matches!(rust_name, "created_at" | "updated_at"),
            update_updated_at: !self.host_managed_timestamp
                && !self.is_relation
                && !self.skip_db
                && self.db_column.as_deref().unwrap_or(rust_name) == "updated_at",
        }
    }
}

fn validate_host_timestamp(
    field: &Field,
    writable_derive: bool,
    inferred_primary_key: bool,
) -> syn::Result<()> {
    let meta = parse_field_metadata(field)?;
    if !meta.host_managed_timestamp {
        return Ok(());
    }
    let invalid = |message| syn::Error::new_spanned(field, message);
    if !writable_derive {
        return Err(invalid(
            "timestamp = \"host\" requires GraphQLEntity or RepositoryEntity",
        ));
    }
    let rust_name = field
        .ident
        .as_ref()
        .expect("named fields validated")
        .to_string();
    let column = meta.db_column.as_deref().unwrap_or(&rust_name);
    if !matches!(rust_name.as_str(), "created_at" | "updated_at")
        && !matches!(column, "created_at" | "updated_at")
    {
        return Err(invalid(
            "host timestamp requires a created_at or updated_at Rust field or physical column",
        ));
    }
    let ty = option_inner_type(&field.ty).unwrap_or(&field.ty);
    if !type_path_last_ident(ty).is_some_and(|ident| ident == "i64" || ident == "String")
        || meta.is_date_field
        || meta.is_boolean_field
        || meta.is_json_field
    {
        return Err(invalid(
            "host timestamp requires an ordinary i64 or String field (optionals allowed)",
        ));
    }
    if meta.is_relation
        || meta.skip_db
        || meta.is_primary_key
        || inferred_primary_key
        || meta.is_version
        || meta.auto_generated == Some(true)
    {
        return Err(invalid(
            "host timestamp cannot be a relation, skipped database field, primary key, version, or generated field",
        ));
    }
    Ok(())
}

pub(crate) fn validate_host_timestamp_fields(
    fields: &syn::punctuated::Punctuated<Field, syn::token::Comma>,
    repository: bool,
) -> syn::Result<()> {
    let explicit_primary_key = fields
        .iter()
        .any(|field| parse_field_metadata(field).is_ok_and(|meta| meta.is_primary_key));
    for field in fields {
        let inferred_primary_key =
            !explicit_primary_key && field.ident.as_ref().is_some_and(|ident| ident == "id");
        validate_host_timestamp(field, repository, inferred_primary_key)?;
    }
    Ok(())
}

pub(crate) fn collect_parsed_fields<'a>(
    fields: impl IntoIterator<Item = &'a Field>,
) -> syn::Result<Vec<ParsedField>> {
    fields
        .into_iter()
        .map(|field| {
            Ok(ParsedField {
                field: field.clone(),
                meta: parse_field_metadata(field)?,
            })
        })
        .collect()
}

fn to_snake_case(s: &str) -> String {
    s.to_case(Case::Snake)
}

fn is_rust_keyword(ident: &str) -> bool {
    matches!(
        ident,
        "as" | "break"
            | "const"
            | "continue"
            | "crate"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "async"
            | "await"
            | "dyn"
            | "abstract"
            | "become"
            | "box"
            | "do"
            | "final"
            | "macro"
            | "override"
            | "priv"
            | "try"
            | "typeof"
            | "unsized"
            | "virtual"
            | "yield"
    )
}

fn rust_ident_from_graphql_name(graphql_name: &str, span: proc_macro2::Span) -> syn::Ident {
    let snake = to_snake_case(graphql_name);
    if is_rust_keyword(&snake) {
        syn::Ident::new_raw(&snake, span)
    } else {
        syn::Ident::new(&snake, span)
    }
}

pub(crate) fn relation_delete_policy_tokens(
    policy: Option<&str>,
    span: proc_macro2::Span,
) -> syn::Result<proc_macro2::TokenStream> {
    match policy.unwrap_or("restrict") {
        "restrict" => Ok(quote! { ::graphql_orm::graphql::orm::DeletePolicy::Restrict }),
        "cascade" => Ok(quote! { ::graphql_orm::graphql::orm::DeletePolicy::Cascade }),
        "set_null" => Ok(quote! { ::graphql_orm::graphql::orm::DeletePolicy::SetNull }),
        other => Err(syn::Error::new(
            span,
            format!(
                "unsupported relation on_delete policy '{other}'; expected 'restrict', 'cascade', or 'set_null'"
            ),
        )),
    }
}

pub(crate) fn relation_change_propagation_tokens(
    propagation: Option<&str>,
    span: proc_macro2::Span,
) -> syn::Result<proc_macro2::TokenStream> {
    match propagation.unwrap_or("none") {
        "none" => Ok(quote! { ::graphql_orm::graphql::orm::RelationChangePropagation::None }),
        "up" => Ok(quote! { ::graphql_orm::graphql::orm::RelationChangePropagation::Up }),
        other => Err(syn::Error::new(
            span,
            format!(
                "unsupported relation propagate_change value '{other}'; expected 'none' or 'up'"
            ),
        )),
    }
}

fn backup_policy_tokens(
    policy: Option<&str>,
    span: proc_macro2::Span,
) -> syn::Result<proc_macro2::TokenStream> {
    match policy.unwrap_or("include") {
        "include" => Ok(quote! { ::graphql_orm::graphql::orm::ColumnBackupPolicy::Include }),
        "exclude" => Ok(quote! { ::graphql_orm::graphql::orm::ColumnBackupPolicy::Exclude }),
        "redact" => Ok(quote! { ::graphql_orm::graphql::orm::ColumnBackupPolicy::Redact }),
        other => Err(syn::Error::new(
            span,
            format!(
                "unsupported backup policy '{other}'; expected 'include', 'exclude', or 'redact'"
            ),
        )),
    }
}

fn backup_value_kind_tokens(ty: &syn::Type, meta: &FieldMetadata) -> proc_macro2::TokenStream {
    if meta.decimal.is_some() {
        return quote! { ::graphql_orm::graphql::orm::BackupValueKind::Decimal };
    }
    if meta.is_json_field {
        return quote! { ::graphql_orm::graphql::orm::BackupValueKind::Json };
    }
    if meta.is_boolean_field {
        return quote! { ::graphql_orm::graphql::orm::BackupValueKind::Bool };
    }
    if meta.is_date_field {
        return quote! { ::graphql_orm::graphql::orm::BackupValueKind::String };
    }
    if is_uuid_type(ty) {
        return quote! { ::graphql_orm::graphql::orm::BackupValueKind::Uuid };
    }
    if is_byte_vec_type(ty) {
        return quote! { ::graphql_orm::graphql::orm::BackupValueKind::Bytes };
    }

    let inner_type = option_inner_type(ty).unwrap_or(ty);
    // `Option<Vec<u8>>` stores BYTEA/BLOB (see the SQL type inference); its logical kind must be
    // Bytes as well, not the generic `Vec` -> Json fallback.
    if is_byte_vec_type(inner_type) {
        return quote! { ::graphql_orm::graphql::orm::BackupValueKind::Bytes };
    }
    if let Some(ident) = type_path_last_ident(inner_type) {
        return match ident.to_string().as_str() {
            "bool" => quote! { ::graphql_orm::graphql::orm::BackupValueKind::Bool },
            "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64" | "usize" => {
                quote! { ::graphql_orm::graphql::orm::BackupValueKind::Integer }
            }
            "f32" | "f64" => quote! { ::graphql_orm::graphql::orm::BackupValueKind::Float },
            "Vec" => quote! { ::graphql_orm::graphql::orm::BackupValueKind::Json },
            _ => quote! { ::graphql_orm::graphql::orm::BackupValueKind::String },
        };
    }

    quote! { ::graphql_orm::graphql::orm::BackupValueKind::String }
}

fn validate_relation_delete_policy(
    struct_name: &syn::Ident,
    field: &Field,
    field_meta: &FieldMetadata,
    parsed_fields: &[ParsedField],
) -> syn::Result<()> {
    if field_meta.relation_on_delete.as_deref() != Some("set_null") {
        return Ok(());
    }

    let source_column = field_meta
        .relation_from
        .clone()
        .unwrap_or_else(|| default_relation_source_field(parsed_fields));
    let source_field = parsed_fields
        .iter()
        .find(|parsed| {
            parsed
                .field
                .ident
                .as_ref()
                .map(|ident| ident == &syn::Ident::new(&source_column, ident.span()))
                .unwrap_or(false)
        })
        .ok_or_else(|| {
            syn::Error::new_spanned(
                field,
                format!(
                    "Relation '{}' references unknown source field '{}' on '{}'",
                    field
                        .ident
                        .as_ref()
                        .map(|ident| ident.to_string())
                        .unwrap_or_default(),
                    source_column,
                    struct_name
                ),
            )
        })?;

    if !is_option_type(&source_field.field.ty) {
        return Err(syn::Error::new_spanned(
            field,
            format!(
                "relation on_delete = \"set_null\" requires nullable source field '{}.{}'",
                struct_name, source_column
            ),
        ));
    }

    Ok(())
}

/// Wrap a value expression with a synchronous read transform.
///
/// The transform function must have the signature: `fn(String) -> String`
///
/// If `transform_read` is None, returns the original expression unchanged.
pub(crate) fn maybe_wrap_read_transform(
    expr: proc_macro2::TokenStream,
    transform_read: &Option<String>,
) -> proc_macro2::TokenStream {
    match transform_read {
        Some(path_str) => {
            let path: syn::Path = syn::parse_str(path_str)
                .unwrap_or_else(|_| syn::parse_str("unknown_transform").unwrap());
            quote! {
                {
                    let __raw_val = #expr;
                    #path(__raw_val)
                }
            }
        }
        None => expr,
    }
}

/// The physical column of an implicit `id` key: the persisted scalar field
/// named `id`, when no field declares `#[primary_key]`.
pub(crate) fn implicit_id_db_column(parsed_fields: &[ParsedField]) -> Option<String> {
    parsed_fields
        .iter()
        .find(|parsed| {
            !parsed.meta.is_relation
                && !parsed.meta.skip_db
                && parsed
                    .field
                    .ident
                    .as_ref()
                    .is_some_and(|ident| ident == "id")
        })
        .map(|parsed| {
            parsed
                .meta
                .db_column
                .clone()
                .unwrap_or_else(|| "id".to_string())
        })
}

/// The Rust field that a relation without `from` uses as its source key: the
/// first `#[primary_key]` field, or the implicit `id` key.
pub(crate) fn default_relation_source_field(parsed_fields: &[ParsedField]) -> String {
    parsed_fields
        .iter()
        .find(|parsed| parsed.meta.is_primary_key)
        .and_then(|parsed| parsed.field.ident.as_ref())
        .map(|ident| ident.to_string())
        .unwrap_or_else(|| "id".to_string())
}

/// `ORDER BY` text for an omitted `default_sort`: every primary-key column in
/// declaration order. `columns` hold each key's identity as `PRIMARY_KEYS`
/// stores it.
fn primary_key_default_sort(backend: BackendKind, columns: &[String]) -> String {
    columns
        .iter()
        .map(|column| backend_render_identifier_path(backend, column))
        .collect::<Vec<_>>()
        .join(", ")
}

// ============================================================================
// GraphQL Entity Code Generation
// ============================================================================

pub(crate) fn generate_graphql_entity(
    input: &DeriveInput,
) -> syn::Result<proc_macro2::TokenStream> {
    generate_entity_impl(input, false, false)
}

/// Reuse the generated identity resolvers inside a composed complex object.
///
/// Returns the resolvers and the GraphQL names of the fields they serve.
pub(crate) fn generate_complex_identity_fields(
    input: &DeriveInput,
) -> syn::Result<(proc_macro2::TokenStream, Vec<&'static str>)> {
    let mut names = Vec::new();
    let fields = generate_entity_tokens(input, false, Some(&mut names))?;
    Ok((fields, names))
}

/// Require the operations derive whenever a Federation key is declared.
///
/// `GraphQLEntity` cannot see the rest of the derive list, so it emits a
/// compile-time bound against a trait that only `GraphQLOperations` implements.
/// This turns "declared a key, generated nothing" into a diagnosed error.
pub(crate) fn federation_key_witness_tokens(
    attrs: &[syn::Attribute],
    struct_name: &syn::Ident,
) -> syn::Result<proc_macro2::TokenStream> {
    let metadata = parse_entity_metadata(attrs)?;
    if metadata.federation_keys.is_empty() && metadata.node_id.is_none() {
        return Ok(quote! {});
    }
    let witness = syn::Ident::new(
        &format!(
            "__GOM_FEDERATION_KEY_WITNESS_{}",
            struct_name.to_string().to_uppercase()
        ),
        struct_name.span(),
    );
    Ok(quote! {
        #[doc(hidden)]
        #[allow(dead_code)]
        const #witness: () = {
            const fn assert_generated_federation_keys<
                T: ::graphql_orm::graphql::federation::GeneratedFederationEntityKeys,
            >() {}
            assert_generated_federation_keys::<#struct_name>();
        };
    })
}

pub(crate) fn generate_graphql_schema_entity(
    input: &DeriveInput,
) -> syn::Result<proc_macro2::TokenStream> {
    generate_entity_impl(input, true, false)
}

fn generate_entity_impl(
    input: &DeriveInput,
    schema_only_override: bool,
    identity_only: bool,
) -> syn::Result<proc_macro2::TokenStream> {
    let mut names = Vec::new();
    generate_entity_tokens(
        input,
        schema_only_override,
        identity_only.then_some(&mut names),
    )
}

/// With `identity_names`, return only the identity resolvers and record their names.
fn generate_entity_tokens(
    input: &DeriveInput,
    schema_only_override: bool,
    identity_names: Option<&mut Vec<&'static str>>,
) -> syn::Result<proc_macro2::TokenStream> {
    let identity_only = identity_names.is_some();
    let struct_name = &input.ident;
    let entity_meta = parse_entity_metadata(&input.attrs)?;
    let backend = resolve_backend(
        entity_meta.backend.as_deref(),
        struct_name.span(),
        "graphql_entity",
    )?;
    if entity_meta.rls.is_some() && backend != BackendKind::Postgres {
        return Err(syn::Error::new_spanned(
            input,
            "#[graphql_rls] is only supported for the Postgres backend",
        ));
    }
    let backend_marker = backend_marker_tokens(backend);
    let row_type = backend_row_type_tokens(backend);
    let helper_import = backend_helper_import_tokens(backend);
    let placeholder_body = if backend == BackendKind::Postgres {
        quote! { format!("${}", index) }
    } else if backend == BackendKind::Mssql {
        quote! { format!("@P{}", index) }
    } else {
        quote! { "?".to_string() }
    };
    let ci_like_body = if backend == BackendKind::Postgres {
        quote! { format!("{} ILIKE {} ESCAPE '\\'", column, placeholder) }
    } else {
        quote! { format!("LOWER({}) LIKE LOWER({}) ESCAPE '\\'", column, placeholder) }
    };
    let bool_sql_value_body = if backend == BackendKind::Postgres || backend == BackendKind::Mssql {
        quote! { ::graphql_orm::graphql::orm::SqlValue::Bool(value) }
    } else {
        quote! { ::graphql_orm::graphql::orm::SqlValue::Int(if value { 1 } else { 0 }) }
    };
    let spatial_sql_value_body = if backend == BackendKind::Sqlite {
        quote! {
            ::graphql_orm::graphql::orm::spatial::canonical_geojson_sql_value(value, spatial)
                .map_err(E::from_sqlx_error)
        }
    } else {
        quote! {
            ::graphql_orm::graphql::orm::json_sql_value::<_, E>(value)
        }
    };

    let data = match &input.data {
        Data::Struct(data) => data,
        _ => {
            return Err(syn::Error::new_spanned(
                input,
                "GraphQLEntity can only be derived for structs",
            ));
        }
    };

    let fields = match &data.fields {
        Fields::Named(fields) => &fields.named,
        _ => {
            return Err(syn::Error::new_spanned(
                input,
                "GraphQLEntity requires named fields",
            ));
        }
    };

    if entity_meta.dependent_deletes {
        if backend == BackendKind::Mssql {
            return Err(syn::Error::new_spanned(
                input,
                "dependent_deletes supports SQLite and PostgreSQL only; MSSQL is unsupported",
            ));
        }
        let key_count = fields
            .iter()
            .filter(|f| parse_field_metadata(f).is_ok_and(|m| m.is_primary_key))
            .count();
        if key_count != 1 {
            return Err(syn::Error::new_spanned(
                input,
                "dependent_deletes requires a single-column primary key; composite keys are unsupported",
            ));
        }
        for field in fields {
            let meta = parse_field_metadata(field)?;
            if meta
                .relation_from_fields
                .as_ref()
                .is_some_and(|f| f.len() > 1)
                || meta
                    .relation_to_fields
                    .as_ref()
                    .is_some_and(|f| f.len() > 1)
            {
                return Err(syn::Error::new_spanned(
                    field,
                    "dependent_deletes does not support composite-key relations",
                ));
            }
        }
    }

    // Generated updates target the physical updated_at column, including aliases.
    let text_updated_at = fields.iter().find_map(|field| {
        field.ident.as_ref().and_then(|ident| {
            let meta = parse_field_metadata(field).ok()?;
            (meta
                .timestamp_behavior(&ident.to_string())
                .update_updated_at
                && is_string_type(&field.ty))
            .then_some(meta.is_date_field)
        })
    });
    let current_updated_at_runtime = if let Some(native_date) = text_updated_at {
        backend_current_timestamp_expr(backend, native_date)
    } else {
        backend_current_epoch_expr(backend)
    };

    let entity_name_lit = struct_name.to_string();
    if backend != BackendKind::Mssql {
        for field in fields {
            if parse_field_metadata(field)?.mssql_money {
                return Err(syn::Error::new_spanned(
                    field,
                    "mssql_money requires backend = \"mssql\"",
                ));
            }
        }
    }
    let schema_only = schema_only_override || entity_meta.schema_only;
    validate_host_timestamp_fields(fields, !schema_only)?;
    if schema_only && !entity_meta.projections.is_empty() {
        return Err(syn::Error::new_spanned(
            input,
            "typed projections require GraphQLEntity; schema-only entities do not generate read APIs",
        ));
    }
    if let (true, Some(span)) = (schema_only, entity_meta.node_id) {
        return Err(syn::Error::new(
            span,
            "node_id requires GraphQLEntity and GraphQLOperations; a schema-only entity has no GraphQL object to resolve",
        ));
    }
    if schema_only && !entity_meta.federation_keys.is_empty() {
        return Err(syn::Error::new(
            entity_meta.federation_keys[0].span,
            "federation_key requires GraphQLEntity and GraphQLOperations; a schema-only entity has no GraphQL object to resolve",
        ));
    }
    if schema_only && entity_meta.retention_policy.is_some() {
        return Err(syn::Error::new_spanned(
            input,
            "retention_purge requires GraphQLEntity and GraphQLOperations generated repository types",
        ));
    }
    if entity_meta.retention_policy.is_some()
        && (backend == BackendKind::Mssql
            || matches!(
                entity_meta.schema_policy.as_deref(),
                Some("external_read_only")
            ))
    {
        return Err(syn::Error::new_spanned(
            struct_name,
            "retention_purge requires a managed writable sqlite or postgres entity",
        ));
    }
    if entity_meta.retention_policy.is_some() {
        for field in fields {
            let field_meta = parse_field_metadata(field)?;
            if field_meta.is_relation
                && field_meta.relation_target.as_deref() == Some(entity_name_lit.as_str())
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
    let backup_enabled = entity_meta.backup_enabled.unwrap_or(true);
    let append_only = entity_meta.append_only;
    let retention_policy = entity_meta
        .retention_policy
        .as_ref()
        .map(|policy| {
            let lit = syn::LitStr::new(policy, struct_name.span());
            quote! { Some(#lit) }
        })
        .unwrap_or_else(|| quote! { None });
    let backup_export_order = entity_meta
        .backup_export_order
        .map(|order| quote! { Some(#order) })
        .unwrap_or_else(|| quote! { None });
    let backup_restore_order = entity_meta
        .backup_restore_order
        .map(|order| quote! { Some(#order) })
        .unwrap_or_else(|| quote! { None });
    let graphql_rename_fields = entity_meta.graphql_rename_fields.as_deref();
    let serde_rename_all = entity_meta.serde_rename_all.as_deref();
    let entity_description = entity_meta
        .description
        .clone()
        .or(semantic_doc_description(&input.attrs)?)
        .unwrap_or_else(|| humanize_identifier(&struct_name.to_string()));
    validate_semantic_description(&entity_description, struct_name.span())?;
    let entity_classification = if entity_meta.classification.is_empty() {
        "internal"
    } else {
        entity_meta.classification.as_str()
    };
    let entity_classification_tokens =
        semantic_classification_tokens(entity_classification, struct_name.span())?;
    let field_case_rule = selected_field_case_rule();
    let read_policy = entity_meta
        .read_policy
        .as_ref()
        .map(|policy| {
            let lit = syn::LitStr::new(policy, struct_name.span());
            quote! { Some(#lit) }
        })
        .unwrap_or_else(|| quote! { None });
    let write_policy = entity_meta
        .write_policy
        .as_ref()
        .map(|policy| {
            let lit = syn::LitStr::new(policy, struct_name.span());
            quote! { Some(#lit) }
        })
        .unwrap_or_else(|| quote! { None });
    let legacy_graphql_complex = has_graphql_complex(&input.attrs)?;
    // Rustc removes derive lists before invoking derives. Valid externally owned
    // objects opt into complex or schema_only, so check those ownership modes.
    // Pure GraphQLSchemaEntity metadata does not own a GraphQL read contract.
    let externally_owned_object = legacy_graphql_complex || entity_meta.schema_only;
    // A `graphql_policy_object` marker must come with the guard that attribute
    // installs; the attribute also implements this witness trait.
    let mut policy_object_witness = quote! {};
    if !schema_only_override && externally_owned_object {
        for field in fields {
            let meta = parse_field_metadata(field)?;
            if !meta.graphql_readable() && !has_graphql_object_skip(field) {
                return Err(syn::Error::new_spanned(
                    field,
                    "unreadable SimpleObject field: private, read = false, and input_only require #[graphql(skip)]; add #[graphql(skip)] to this field or place #[graphql_orm::graphql_policy_object] before the derives with #[graphql(complex)]",
                ));
            }
            // SimpleObject getters apply no ORM column policy by themselves.
            if meta.read_policy.is_some()
                && !meta.is_relation
                && !meta.graphql_skip
                && !has_graphql_object_skip(field)
            {
                if !has_policy_object_marker(&input.attrs) {
                    return Err(syn::Error::new_spanned(
                        field,
                        "SimpleObject field read_policy is not enforced by SimpleObject; place #[graphql_orm::graphql_policy_object] before the derives with #[graphql(complex)], or add #[graphql(skip)] to this field",
                    ));
                }
                policy_object_witness = quote! {
                    const _: fn() = ::graphql_orm::graphql::__policy_object::assert_installed::<#struct_name>;
                };
            }
        }
    }
    let raw_table_name = entity_meta.table_name.as_deref().unwrap_or("unknown");
    let table_name = backend_quote_identifier_path(backend, raw_table_name);
    if let Some(rls) = entity_meta.rls.as_ref() {
        validate_rls_columns(rls, fields.iter())?;
    }
    let rls_impl = rls_impl_tokens(
        struct_name,
        &struct_name.to_string(),
        &table_name,
        entity_meta.rls.as_ref(),
        entity_meta.retention_policy.is_some(),
    );
    let plural_name = entity_meta
        .plural_name
        .as_deref()
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{}s", struct_name));
    let composite_unique_indexes = entity_meta
        .unique_composite
        .iter()
        .map(|cols| {
            let col_lits = cols
                .iter()
                .map(|c| syn::LitStr::new(c, struct_name.span()))
                .collect::<Vec<_>>();
            quote! { &[#(#col_lits),*] }
        })
        .collect::<Vec<_>>();
    let mut index_defs = Vec::new();
    for index in &entity_meta.indexes {
        let mut db_columns = Vec::new();
        for requested in &index.columns {
            let field = fields
                .iter()
                .find(|field| field.ident.as_ref().is_some_and(|ident| ident == requested))
                .ok_or_else(|| {
                    syn::Error::new_spanned(
                        struct_name,
                        format!("index references unknown field `{requested}`"),
                    )
                })?;
            let field_meta = parse_field_metadata(field)?;
            if field_meta.is_relation || field_meta.skip_db {
                return Err(syn::Error::new_spanned(
                    field,
                    "index columns must be persisted scalar fields",
                ));
            }
            db_columns.push(field_meta.db_column.unwrap_or_else(|| requested.clone()));
        }
        let name = index.name.clone().unwrap_or_else(|| {
            bounded_generated_identifier(format!("idx_{}_{}", raw_table_name, db_columns.join("_")))
        });
        let columns = db_columns
            .iter()
            .map(|column| syn::LitStr::new(column, struct_name.span()))
            .collect::<Vec<_>>();
        let directions = index
            .directions
            .iter()
            .map(|direction| match direction {
                OrdinaryIndexDirection::Asc => {
                    quote! { ::graphql_orm::graphql::orm::IndexDirection::Asc }
                }
                OrdinaryIndexDirection::Desc => {
                    quote! { ::graphql_orm::graphql::orm::IndexDirection::Desc }
                }
            })
            .collect::<Vec<_>>();
        let base = if index.unique {
            quote! {
                ::graphql_orm::graphql::orm::IndexDef::new(#name, &[#(#columns),*]).unique()
            }
        } else {
            quote! {
                ::graphql_orm::graphql::orm::IndexDef::new(#name, &[#(#columns),*])
            }
        };
        if directions.is_empty() {
            index_defs.push(base);
        } else {
            index_defs.push(quote! { #base.with_directions(&[#(#directions),*]) });
        }
    }

    for conditional in &entity_meta.conditional_indexes {
        if backend == BackendKind::Mssql {
            return Err(syn::Error::new_spanned(
                struct_name,
                "conditional indexes are supported only for sqlite and postgres",
            ));
        }
        let mut db_columns = Vec::new();
        for requested in &conditional.columns {
            let field = fields
                .iter()
                .find(|field| field.ident.as_ref().is_some_and(|ident| ident == requested))
                .ok_or_else(|| {
                    syn::Error::new_spanned(
                        struct_name,
                        format!("conditional_index references unknown field `{requested}`"),
                    )
                })?;
            let meta = parse_field_metadata(field)?;
            if meta.is_relation || meta.skip_db {
                return Err(syn::Error::new_spanned(
                    field,
                    "conditional_index columns must be persisted scalar fields",
                ));
            }
            db_columns.push(meta.db_column.unwrap_or_else(|| requested.clone()));
        }
        let predicate_rust_field = conditional
            .predicate_field
            .as_deref()
            .expect("validated conditional predicate field");
        let predicate_field = fields
            .iter()
            .find(|field| {
                field
                    .ident
                    .as_ref()
                    .is_some_and(|ident| ident == predicate_rust_field)
            })
            .ok_or_else(|| {
                syn::Error::new_spanned(
                    struct_name,
                    format!(
                        "conditional_index references unknown predicate field `{predicate_rust_field}`"
                    ),
                )
            })?;
        let predicate_meta = parse_field_metadata(predicate_field)?;
        let predicate_type = option_inner_type(&predicate_field.ty).unwrap_or(&predicate_field.ty);
        if predicate_meta.is_relation || predicate_meta.skip_db || !is_string_type(predicate_type) {
            return Err(syn::Error::new_spanned(
                predicate_field,
                "conditional_index predicate_field must be a persisted String or Option<String>",
            ));
        }
        let predicate_column = predicate_meta
            .db_column
            .unwrap_or_else(|| predicate_rust_field.to_string());
        let mut values = conditional.predicate_values.clone();
        values.sort();
        values.dedup();
        if values.iter().any(String::is_empty) {
            return Err(syn::Error::new_spanned(
                predicate_field,
                "conditional_index predicate values cannot be empty",
            ));
        }
        let name = conditional.name.clone().unwrap_or_else(|| {
            format!(
                "{}_{}_{}_where_{}",
                if conditional.unique { "uidx" } else { "idx" },
                raw_table_name.replace('.', "_"),
                db_columns.join("_"),
                predicate_column
            )
        });
        let columns = db_columns
            .iter()
            .map(|column| syn::LitStr::new(column, struct_name.span()))
            .collect::<Vec<_>>();
        let values = values
            .iter()
            .map(|value| syn::LitStr::new(value, struct_name.span()))
            .collect::<Vec<_>>();
        let base = if conditional.unique {
            quote! { ::graphql_orm::graphql::orm::IndexDef::new(#name, &[#(#columns),*]).unique() }
        } else {
            quote! { ::graphql_orm::graphql::orm::IndexDef::new(#name, &[#(#columns),*]) }
        };
        index_defs.push(quote! { #base.where_in(#predicate_column, &[#(#values),*]) });
    }

    // Collect field info
    let mut column_names: Vec<String> = Vec::new();
    let mut column_defs: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut constraint_defs: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut primary_key_cols: Vec<String> = Vec::new();
    let mut where_input_fields = Vec::new();
    let mut order_by_fields = Vec::new();
    let mut filter_to_sql = Vec::new();
    let mut filter_to_entity_match = Vec::new();
    let mut filter_is_empty_checks = Vec::new();
    let mut related_prepare = Vec::new();
    let mut related_render = Vec::new();
    let mut related_validate = Vec::new();
    let mut related_present = Vec::new();
    let mut filter_contains_spatial_checks = Vec::new();
    let mut filter_validation_checks = Vec::new();
    let mut filter_referenced_field_checks = Vec::new();
    let mut from_row_fields = Vec::new();
    let mut relation_metadata_defs = Vec::new();
    let mut search_field_defs = Vec::new();
    let mut search_json_path_defs = Vec::new();
    let mut search_relation_defs = Vec::new();
    let mut search_document_chunks = Vec::new();
    let mut sortable_columns: Vec<(syn::Ident, String)> = Vec::new();
    let mut order_expression_parameter_arms = Vec::new();
    let mut order_referenced_field_checks = Vec::new();
    let mut search_relation_policy_tokens = Vec::new();
    let mut order_expression_context_checks = Vec::new();
    let mut relation_order_match_arms = Vec::new();
    let mut relation_order_value_arms = Vec::new();
    let mut relation_order_validate = Vec::new();
    let mut relation_order_present = Vec::new();
    let mut relation_order_prepare = Vec::new();
    let mut order_by_graphql_names = std::collections::BTreeSet::new();
    let mut order_by_rust_names = std::collections::BTreeSet::new();
    let mut object_field_methods = Vec::new();
    let mut identity_field_methods = Vec::new();
    let mut identity_field_names = Vec::new();
    // GraphQL name of the managed record version field, also served as
    // `RecordVersion`; none for CAS-only versions and unversioned entities.
    let mut managed_version_api_name: Option<String> = None;
    let mut repository_field_policy_defs = Vec::new();
    let mut persisted_field_rust_names = Vec::new();
    let mut search_relation_field_checks = Vec::new();
    let mut search_relation_traversal_checks = Vec::new();
    let mut subscription_field_policy_arms = Vec::new();
    let mut semantic_field_defs = Vec::new();
    let hidden_input_member_defs = generated_hidden_input_members(
        struct_name,
        fields,
        &entity_meta,
        entity_classification,
        graphql_rename_fields,
        serde_rename_all,
    )?;
    let mut semantic_record_version = quote! { None };
    let mut aggregate_field_variants = Vec::new();
    let mut aggregate_field_match_arms = Vec::new();
    let parsed_fields = collect_parsed_fields(fields.iter())?;
    let relation_readability_columns = parsed_fields
        .iter()
        .filter(|parsed| !parsed.meta.is_relation && !parsed.meta.skip_db)
        .map(|parsed| {
            let rust_name = parsed.field.ident.as_ref().unwrap().to_string();
            let column = relation_column_identity(
                backend,
                parsed.meta.db_column.as_deref().unwrap_or(&rust_name),
                false,
            );
            let readable = parsed.meta.graphql_readable();
            quote! { (#column, #readable) }
        });
    let relation_readability_impl = quote! {
        impl ::graphql_orm::graphql::orm::GeneratedRelationReadability for #struct_name {
            const COLUMNS: &'static [(&'static str, bool)] = &[#(#relation_readability_columns),*];
        }
    };
    let mut relation_target_checks = Vec::new();
    let aggregate_field_name = syn::Ident::new(
        &format!("{}AggregateField", struct_name),
        struct_name.span(),
    );

    for parsed_field in &parsed_fields {
        let field = &parsed_field.field;
        let field_name = field.ident.as_ref().unwrap();
        let field_type = &field.ty;
        let field_meta = parsed_field.meta.clone();

        if field_meta.search_relation.is_some() && !field_meta.is_relation {
            return Err(syn::Error::new(
                field.span(),
                "graphql_orm(search_relation(...)) requires a #[relation(...)] field",
            ));
        }
        if field_meta.search.is_some() && (field_meta.is_relation || field_meta.skip_db) {
            return Err(syn::Error::new(
                field.span(),
                "graphql_orm(searchable(...)) requires a persisted scalar field",
            ));
        }
        if !field_meta.search_json.is_empty() && (field_meta.is_relation || field_meta.skip_db) {
            return Err(syn::Error::new(
                field.span(),
                "graphql_orm(search_json(...)) requires a persisted JSON field",
            ));
        }

        let subscription_api_name = graphql_field_name(
            &field_meta,
            &field_name.to_string(),
            graphql_rename_fields,
            serde_rename_all,
        );
        let subscription_allowed = !has_repository_entity_attribute(&input.attrs)
            && field_meta.graphql_readable()
            && (!field_meta.graphql_skip || field_meta.is_relation)
            && field_meta.subscribe
            && !field_meta.subscription_custom_resolver;
        let subscription_read_policy = field_meta
            .read_policy
            .as_ref()
            .map(|policy| quote! { Some(#policy) })
            .unwrap_or_else(|| quote! { None });
        subscription_field_policy_arms.push(quote! {
            #subscription_api_name => Some((#subscription_api_name, #subscription_allowed, #subscription_read_policy)),
        });
        if subscription_api_name == RECORD_VERSION_FIELD
            && parsed_fields
                .iter()
                .any(|parsed| parsed.meta.managed_version)
        {
            return Err(syn::Error::new(
                field.span(),
                "this entity has a managed record version, which is also served as the generated `RecordVersion` field; rename this field",
            ));
        }
        if field_meta.managed_version
            && field_meta.graphql_readable()
            && !field_meta.graphql_skip
            && !has_repository_entity_attribute(&input.attrs)
        {
            managed_version_api_name = Some(subscription_api_name.clone());
            subscription_field_policy_arms.push(quote! {
                #RECORD_VERSION_FIELD => Some((#subscription_api_name, #subscription_allowed, #subscription_read_policy)),
            });
        }

        // Skip relation fields for column list
        if field_meta.is_relation || field_meta.skip_db {
            if field_meta.is_relation {
                validate_relation_delete_policy(struct_name, field, &field_meta, &parsed_fields)?;
                let emits_foreign_key = field_meta
                    .relation_emit_foreign_key
                    .unwrap_or(!field_meta.relation_multiple);
                if (field_meta.relation_source_condition.is_some()
                    || field_meta.relation_target_condition.is_some())
                    && emits_foreign_key
                {
                    return Err(syn::Error::new_spanned(
                        field,
                        "conditional relations require `emit_fk = false` because they do not describe an unconditional physical foreign key",
                    ));
                }
                let rust_name = field_name.to_string();
                let graphql_name = graphql_field_name(
                    &field_meta,
                    &rust_name,
                    graphql_rename_fields,
                    serde_rename_all,
                );
                let target_type = field_meta
                    .relation_target
                    .clone()
                    .unwrap_or_else(|| "Unknown".to_string());
                if field_meta.graphql_readable() {
                    let description = field_meta
                        .description
                        .as_deref()
                        .expect("field descriptions are resolved during parsing");
                    let is_multiple = field_meta.relation_multiple;
                    let classification = field_meta
                        .classification
                        .as_deref()
                        .unwrap_or(entity_classification);
                    if semantic_classification_rank(classification)
                        < semantic_classification_rank(entity_classification)
                    {
                        return Err(syn::Error::new_spanned(
                            field,
                            "field classification cannot weaken its entity classification",
                        ));
                    }
                    let classification_tokens =
                        semantic_classification_tokens(classification, field.span())?;
                    let export_tokens = if field_meta.non_exportable || classification == "secret" {
                        quote! { ::graphql_orm::graphql::orm::GraphqlSemanticExport::NeverExport }
                    } else {
                        quote! { ::graphql_orm::graphql::orm::GraphqlSemanticExport::Exportable }
                    };
                    let relationship_type = if is_multiple {
                        quote! {
                            ::graphql_orm::graphql::orm::GraphqlSemanticTypeRef::list(
                                false,
                                Some(::graphql_orm::graphql::orm::PaginationConfig::DEFAULT_MAX_LIMIT as u32),
                                ::graphql_orm::graphql::orm::GraphqlSemanticTypeRef::named(
                                    #target_type,
                                    ::graphql_orm::graphql::orm::GraphqlSemanticTypeKind::Object,
                                    false,
                                ),
                            )
                        }
                    } else {
                        quote! {
                            ::graphql_orm::graphql::orm::GraphqlSemanticTypeRef::named(
                                #target_type,
                                ::graphql_orm::graphql::orm::GraphqlSemanticTypeKind::Object,
                                true,
                            )
                        }
                    };
                    let cardinality = if is_multiple {
                        quote! { ::graphql_orm::graphql::orm::GraphqlSemanticRelationshipCardinality::Many }
                    } else {
                        quote! { ::graphql_orm::graphql::orm::GraphqlSemanticRelationshipCardinality::One }
                    };
                    let (relationship_arguments, collection_bound) = if is_multiple {
                        let contract = GeneratedRelationshipArgumentContract::to_many(
                            &target_type,
                            field.span(),
                        );
                        let page_name = contract.page_name().to_owned();
                        (
                            contract.semantic_descriptors(),
                            quote! {
                                Some(::graphql_orm::graphql::orm::GraphqlSemanticCollectionBound::pageable(#page_name))
                            },
                        )
                    } else {
                        (quote! { vec![] }, quote! { None })
                    };
                    let has_field_policy = field_meta.read_policy.is_some();
                    semantic_field_defs.push(quote! {
                        ::graphql_orm::graphql::orm::GraphqlSemanticFieldMetadata {
                            field_name: ::std::string::ToString::to_string(#graphql_name),
                            description: ::std::string::ToString::to_string(#description),
                            type_ref: #relationship_type,
                            selectable: true,
                            filter_operators: vec![],
                            filter_input: None,
                            sortable: false,
                            groupable: false,
                            aggregate_operators: vec![],
                            aggregate_value_kind: None,
                            relationship: Some(::graphql_orm::graphql::orm::GraphqlSemanticRelationshipDescriptor {
                                target: ::std::string::ToString::to_string(#target_type),
                                cardinality: #cardinality,
                                arguments: #relationship_arguments,
                                collection_bound: #collection_bound,
                            }),
                            classification: #classification_tokens,
                            export: #export_tokens,
                            has_field_policy: #has_field_policy,
                        }
                    });
                }
                let source_columns = field_meta.relation_from_fields.clone().unwrap_or_else(|| {
                    vec![
                        field_meta
                            .relation_from
                            .clone()
                            .unwrap_or_else(|| default_relation_source_field(&parsed_fields)),
                    ]
                });
                let target_columns = field_meta.relation_to_fields.clone().unwrap_or_else(|| {
                    vec![
                        field_meta
                            .relation_to
                            .clone()
                            .unwrap_or_else(|| "unknown_id".to_string()),
                    ]
                });
                if source_columns.len() != target_columns.len() {
                    return Err(syn::Error::new_spanned(
                        field,
                        format!(
                            "Relation '{}' has {} source key part(s) but {} target key part(s)",
                            rust_name,
                            source_columns.len(),
                            target_columns.len()
                        ),
                    ));
                }
                for source_column in &source_columns {
                    if !parsed_fields.iter().any(|parsed| {
                        parsed
                            .field
                            .ident
                            .as_ref()
                            .is_some_and(|ident| ident == source_column)
                    }) {
                        return Err(syn::Error::new_spanned(
                            field,
                            format!(
                                "Relation '{}' references unknown source field '{}' on '{}'",
                                rust_name, source_column, struct_name
                            ),
                        ));
                    }
                }
                if !schema_only_override && !has_repository_entity_attribute(&input.attrs) {
                    validate_graphql_relation_keys(
                        backend,
                        struct_name,
                        field,
                        &field_meta,
                        &parsed_fields,
                        &source_columns,
                        &target_columns,
                    )?;
                    relation_target_checks.push(graphql_relation_target_checks(
                        backend,
                        struct_name,
                        field,
                        &field_meta,
                        &target_columns,
                    )?);
                }
                let source_column = source_columns
                    .first()
                    .cloned()
                    .unwrap_or_else(|| default_relation_source_field(&parsed_fields));
                let target_column = target_columns
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "unknown_id".to_string());
                let source_columns_tokens = source_columns.iter().collect::<Vec<_>>();
                let target_columns_tokens = target_columns.iter().collect::<Vec<_>>();
                let is_multiple = field_meta.relation_multiple;
                let emit_foreign_key = field_meta.relation_emit_foreign_key.unwrap_or(!is_multiple);
                let on_delete = relation_delete_policy_tokens(
                    field_meta.relation_on_delete.as_deref(),
                    field.span(),
                )?;
                let propagate_change = relation_change_propagation_tokens(
                    field_meta.relation_propagate_change.as_deref(),
                    field.span(),
                )?;
                let search_fields_tokens = if let Some(search_relation) =
                    &field_meta.search_relation
                {
                    if !field_meta.graphql_readable() {
                        return Err(syn::Error::new_spanned(
                            field,
                            "search_relation requires a GraphQL-readable relation",
                        ));
                    }
                    let search_weight =
                        search_weight_tokens(&search_relation.weight, field.span())?;
                    let search_fields = search_relation
                        .fields
                        .iter()
                        .map(|field| syn::LitStr::new(field, field_name.span()))
                        .collect::<Vec<_>>();
                    let max_items = search_relation.max_items;
                    let policy = option_lit_tokens(search_relation.policy.as_deref(), field.span());
                    let search_target = syn::Ident::new(&target_type, field.span());
                    let relation_read_source = field_meta.read_policy.as_deref().map(|key| {
                        quote! {
                            sources.push(::graphql_orm::graphql::orm::SearchSourcePolicy::Field {
                                entity_name: #entity_name_lit,
                                field_name: #graphql_name,
                                policy_key: Some(#key),
                            });
                        }
                    });
                    let relation_search_source = search_relation.policy.as_deref().map(|key| {
                        quote! {
                            sources.push(::graphql_orm::graphql::orm::SearchSourcePolicy::Field {
                                entity_name: #entity_name_lit,
                                field_name: #graphql_name,
                                policy_key: Some(#key),
                            });
                        }
                    });
                    // The target is another derive's output: check each copied
                    // field against its persisted field names at compile time.
                    for search_field in &search_fields {
                        let message = format!(
                            "search_relation field `{}` is not a persisted field of `{}`",
                            search_field.value(),
                            target_type,
                        );
                        search_relation_field_checks.push(
                            ::quote::quote_spanned! {search_field.span()=>
                                const _: () = ::std::assert!(
                                    ::graphql_orm::graphql::orm::generated_field_names_contain(
                                        #search_target::__GOM_PERSISTED_FIELD_NAMES,
                                        #search_field,
                                    ),
                                    #message,
                                );
                            },
                        );
                    }
                    search_relation_traversal_checks.push(quote! {
                        ::graphql_orm::graphql::orm::authorize_relation_target::<#search_target>(ctx, #is_multiple)?;
                    });
                    search_relation_policy_tokens.push(quote! {
                        #relation_read_source
                        #relation_search_source
                        sources.push(::graphql_orm::graphql::orm::SearchSourcePolicy::Entity {
                            entity_name: <#search_target as ::graphql_orm::graphql::orm::Entity>::entity_name(),
                            read_policy: <#search_target as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                        });
                        for rust_name in [#(#search_fields),*] {
                            match <#search_target as ::graphql_orm::graphql::orm::Entity>::repository_field_policies()
                                .iter()
                                .find(|field| field.rust_name == rust_name)
                            {
                                Some(field) => sources.push(::graphql_orm::graphql::orm::SearchSourcePolicy::Field {
                                    entity_name: <#search_target as ::graphql_orm::graphql::orm::Entity>::entity_name(),
                                    field_name: field.api_name,
                                    policy_key: field.read_policy,
                                }),
                                None => sources.push(::graphql_orm::graphql::orm::SearchSourcePolicy::Unresolved),
                            }
                        }
                    });
                    search_relation_defs.push(quote! {
                        ::graphql_orm::graphql::orm::SearchRelationFieldDef {
                            relation_field: #graphql_name,
                            target_type: #target_type,
                            fields: &[#(#search_fields),*],
                            weight: #search_weight,
                            max_items: #max_items,
                            policy: #policy,
                        }
                    });
                    quote! {
                        Some(::graphql_orm::graphql::orm::SearchRelationFieldDef {
                            relation_field: #graphql_name,
                            target_type: #target_type,
                            fields: &[#(#search_fields),*],
                            weight: #search_weight,
                            max_items: #max_items,
                            policy: #policy,
                        })
                    }
                } else {
                    quote! { None }
                };

                // Sibling derives are erased by rustc. Only the relation derive's
                // concrete output witness can prove an implementation skip is safe.
                let delegated_readable =
                    scalar_output_is_readable(field, &field_meta, false, &input.attrs)
                        && (schema_only || legacy_graphql_complex);
                let generated_readable =
                    scalar_output_is_readable(field, &field_meta, true, &input.attrs);
                let has_source_condition = field_meta.relation_source_condition.is_some();
                let has_target_condition = field_meta.relation_target_condition.is_some();
                relation_metadata_defs.push(quote! {
                    ::graphql_orm::graphql::orm::RelationMetadata {
                        has_source_condition: #has_source_condition,
                        has_target_condition: #has_target_condition,
                        is_readable: #delegated_readable || (#generated_readable && _generated_output.contains(&#graphql_name)),
                        field_name: #graphql_name,
                        target_type: #target_type,
                        source_column: #source_column,
                        target_column: #target_column,
                        source_columns: &[#(#source_columns_tokens),*],
                        target_columns: &[#(#target_columns_tokens),*],
                        is_multiple: #is_multiple,
                        emit_foreign_key: #emit_foreign_key,
                        on_delete: #on_delete,
                        propagate_change: #propagate_change,
                        search_fields: #search_fields_tokens,
                    }
                });

                if field_meta.relation_filter {
                    if !is_multiple
                        || !field_meta.graphql_readable()
                        || field_meta.read_policy.is_some()
                        || field_meta.relation_source_condition.is_some()
                        || field_meta.relation_target_condition.is_some()
                    {
                        return Err(syn::Error::new_spanned(
                            field,
                            "relation(filter) requires an unconditional public collection without a field policy",
                        ));
                    }
                    let target = syn::Ident::new(&target_type, field.span());
                    let target_where =
                        syn::Ident::new(&format!("{target_type}WhereInput"), field.span());
                    let source_physical = source_columns
                        .iter()
                        .map(|name| {
                            let source = parsed_fields
                                .iter()
                                .find(|p| p.field.ident.as_ref().is_some_and(|i| i == name))
                                .unwrap();
                            source.meta.db_column.as_deref().unwrap_or(name).to_owned()
                        })
                        .collect::<Vec<_>>();
                    where_input_fields.push(quote! {
                        /// Match any authorized related row satisfying one of these alternatives; an empty list matches nothing.
                        #[graphql(name = #graphql_name)]
                        pub #field_name: Option<Vec<#target_where>>,
                    });
                    filter_is_empty_checks
                        .push(quote! { if self.#field_name.is_some() { return false; } });
                    related_present.push(quote! { self.#field_name.is_some() });
                    related_validate.push(quote! {
                        if self.#field_name.is_some() && !self.__gom_related.contains_key(#graphql_name) {
                            return Err(::graphql_orm::sqlx::Error::Protocol("relation filter requires request authorization".into()));
                        }
                    });
                    related_render.push(quote! {
                        if self.#field_name.is_some() {
                            if let Some((clauses, binds)) = self.__gom_related.get(#graphql_name) {
                                conditions.extend(clauses.clone());
                                values.extend(binds.clone());
                            }
                        }
                    });
                    related_prepare.push(quote! {
                        if let Some(alternatives) = self.#field_name.as_mut() {
                            ::graphql_orm::graphql::orm::ensure_rls_request_context::<#target, #backend_marker>(
                                ctx,
                                <#target as ::graphql_orm::graphql::orm::Entity>::entity_name(),
                            )?;
                            ctx.data::<::graphql_orm::db::Database<#backend_marker>>()?
                                .ensure_readable_field(ctx, <#struct_name as ::graphql_orm::graphql::orm::Entity>::entity_name(), #graphql_name, None, None).await
                                .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                            for filter in alternatives.iter_mut() {
                                filter.__gom_prepare_read(ctx, depth + 1, budget).await?;
                            }
                            let predicate = ::graphql_orm::graphql::orm::prepare_static_relation_filter::<
                                #backend_marker, #struct_name, #target, #target_where
                            >(ctx, &[#((#source_physical, #target_columns)),*], alternatives).await?;
                            self.__gom_related.insert(#graphql_name.to_owned(), predicate);
                        }
                    });
                }

                if let Some(order_aggregate) = &field_meta.relation_order_aggregate {
                    if field_meta.relation_source_condition.is_some()
                        || field_meta.relation_target_condition.is_some()
                    {
                        return Err(syn::Error::new_spanned(
                            field,
                            "relation order_aggregate is not supported on conditional relations",
                        ));
                    }
                    if !field_meta.graphql_readable() {
                        return Err(syn::Error::new_spanned(
                            field,
                            "relation order_aggregate requires a readable public relation",
                        ));
                    }
                    // A field read policy is decided per parent record, but an
                    // ORDER BY term ranks every parent in one SQL statement, so
                    // the policy cannot be enforced on the count. Refuse rather
                    // than order by counts the caller may not be allowed to read.
                    if field_meta.read_policy.is_some() {
                        return Err(syn::Error::new_spanned(
                            field,
                            "relation order_aggregate cannot be combined with a relation read_policy; per-record field policies cannot be enforced on an ORDER BY count",
                        ));
                    }
                    debug_assert_eq!(order_aggregate.aggregate, "count");
                    if !order_by_graphql_names.insert(order_aggregate.name.clone()) {
                        return Err(syn::Error::new_spanned(
                            field,
                            format!(
                                "duplicate generated ordering field `{}`",
                                order_aggregate.name
                            ),
                        ));
                    }
                    let aggregate_name = &order_aggregate.name;
                    let order_field_name =
                        rust_ident_from_graphql_name(aggregate_name, field.span());
                    if !order_by_rust_names.insert(order_field_name.to_string()) {
                        return Err(syn::Error::new_spanned(
                            field,
                            format!(
                                "ordering field `{aggregate_name}` conflicts with another generated Rust order-input field"
                            ),
                        ));
                    }
                    let target_type = syn::Ident::new(&target_type, field.span());
                    let relation_alias = "__graphql_orm_order_relation";
                    let dialect = backend_dialect_tokens(backend);
                    let predicates = source_columns
                        .iter()
                        .zip(target_columns.iter())
                        .map(|(source_column, target_column)| {
                            let source = parsed_fields
                                .iter()
                                .find(|parsed| {
                                    parsed
                                        .field
                                        .ident
                                        .as_ref()
                                        .is_some_and(|ident| ident == source_column)
                                })
                                .expect("relation source columns were validated above");
                            let source_physical =
                                source.meta.db_column.as_deref().unwrap_or(source_column);
                            format!(
                                "{relation_alias}.{} = {}.{}",
                                backend_render_identifier_path(backend, target_column),
                                backend_render_identifier_path(backend, raw_table_name),
                                backend_render_identifier_path(backend, source_physical),
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(" AND ");
                    // Unprepared inputs render a count that matches no rows and
                    // fail `validate_order`, so no builder executes them.
                    let unauthorized_format = format!(
                        "(SELECT COUNT(*) FROM {{}} AS {relation_alias} WHERE {predicates} AND 1 = 0) {{}}"
                    );
                    relation_order_match_arms.push(quote! {
                        if let Some(dir) = &self.#order_field_name {
                            match self.__gom_relation_counts.get(#aggregate_name) {
                                Some((expression, _)) => parts.push(format!("{} {}", expression, dir.to_sql())),
                                None => parts.push(format!(
                                    #unauthorized_format,
                                    #dialect.render_identifier_path(
                                        <#target_type as ::graphql_orm::graphql::orm::DatabaseEntity>::TABLE_NAME,
                                    ),
                                    dir.to_sql(),
                                )),
                            }
                        }
                    });
                    relation_order_value_arms.push(quote! {
                        if self.#order_field_name.is_some() {
                            if let Some((_, binds)) = self.__gom_relation_counts.get(#aggregate_name) {
                                values.extend(binds.iter().cloned());
                            }
                        }
                    });
                    relation_order_validate.push(quote! {
                        if self.#order_field_name.is_some() && !self.__gom_relation_counts.contains_key(#aggregate_name) {
                            return Err(::graphql_orm::sqlx::Error::Protocol(
                                "relation count ordering requires request authorization".into(),
                            ));
                        }
                    });
                    relation_order_present.push(quote! { self.#order_field_name.is_some() });
                    relation_order_prepare.push(quote! {
                        if self.#order_field_name.is_some() {
                            // The count reads the target table: an RLS target
                            // needs request database authorization.
                            ::graphql_orm::graphql::orm::ensure_rls_request_context::<#target_type, #backend_marker>(
                                ctx,
                                <#target_type as ::graphql_orm::graphql::orm::Entity>::entity_name(),
                            )?;
                            ctx.data::<::graphql_orm::db::Database<#backend_marker>>()?
                                .ensure_readable_field(ctx, <#struct_name as ::graphql_orm::graphql::orm::Entity>::entity_name(), #graphql_name, None, None).await
                                .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                            let prepared = ::graphql_orm::graphql::orm::prepare_relation_count_order::<
                                #backend_marker, #target_type
                            >(ctx, #relation_alias, #predicates).await?;
                            self.__gom_relation_counts.insert(#aggregate_name.to_owned(), prepared);
                        }
                    });
                    order_by_fields.push(quote! {
                        #[graphql(name = #aggregate_name)]
                        pub #order_field_name: Option<::graphql_orm::graphql::orm::OrderDirection>,
                    });
                }
            }

            // Initialize relation fields to empty
            if is_vec_type(field_type) {
                from_row_fields.push(quote! { #field_name: Vec::new(), });
            } else if is_option_type(field_type) {
                from_row_fields.push(quote! { #field_name: None, });
            } else {
                from_row_fields.push(quote! { #field_name: Default::default(), });
            }
            continue;
        }

        let rust_name = field_name.to_string();
        let graphql_name = graphql_field_name(
            &field_meta,
            &rust_name,
            graphql_rename_fields,
            serde_rename_all,
        );
        if field_meta.graphql_readable() {
            let description = field_meta
                .description
                .as_deref()
                .expect("field descriptions are resolved during parsing");
            let classification = field_meta
                .classification
                .as_deref()
                .unwrap_or(entity_classification);
            if semantic_classification_rank(classification)
                < semantic_classification_rank(entity_classification)
            {
                return Err(syn::Error::new_spanned(
                    field,
                    "field classification cannot weaken its entity classification",
                ));
            }
            let classification_tokens =
                semantic_classification_tokens(classification, field.span())?;
            let export_tokens = if field_meta.non_exportable || classification == "secret" {
                quote! { ::graphql_orm::graphql::orm::GraphqlSemanticExport::NeverExport }
            } else {
                quote! { ::graphql_orm::graphql::orm::GraphqlSemanticExport::Exportable }
            };
            let type_name =
                type_path_last_ident(option_inner_type(field_type).unwrap_or(field_type))
                    .map(ToString::to_string)
                    .unwrap_or_default();
            let is_numeric = matches!(
                type_name.as_str(),
                "i8" | "i16"
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
                    | "Decimal"
            );
            let is_float = matches!(type_name.as_str(), "f32" | "f64");
            let is_decimal = type_name == "Decimal";
            let filter_input = if field_meta.filter {
                match field_meta.filterable.as_deref() {
                    Some(kind) => {
                        let helper = match kind {
                            "string" => "StringFilter",
                            "uuid" => "UuidFilter",
                            "number" => "IntFilter",
                            "int64" => "Int64Filter",
                            "decimal" => "DecimalFilter",
                            "boolean" | "bool" => "BoolFilter",
                            "bytes" => "BytesFilter",
                            "date" => "DateFilter",
                            "spatial" => "SpatialFilter",
                            _ => {
                                return Err(syn::Error::new_spanned(
                                    field,
                                    "unsupported canonical filter type",
                                ));
                            }
                        };
                        let helper = syn::Ident::new(helper, field.span());
                        quote! { Some(::graphql_orm::graphql::filters::#helper::graphql_semantic_filter_input()) }
                    }
                    None => quote! { None },
                }
            } else {
                quote! { None }
            };
            let groupable = !field_meta.is_json_field
                && field_meta.spatial.is_none()
                && !is_byte_vec_type(field_type);
            let is_boolean = type_name == "bool" || field_meta.is_boolean_field;
            let aggregate_operators = if is_numeric {
                quote! { vec![
                    ::graphql_orm::graphql::orm::GraphqlAggregateOperator::Count,
                    ::graphql_orm::graphql::orm::GraphqlAggregateOperator::Min,
                    ::graphql_orm::graphql::orm::GraphqlAggregateOperator::Max,
                    ::graphql_orm::graphql::orm::GraphqlAggregateOperator::Sum,
                ] }
            } else if groupable && !is_boolean {
                quote! { vec![
                    ::graphql_orm::graphql::orm::GraphqlAggregateOperator::Count,
                    ::graphql_orm::graphql::orm::GraphqlAggregateOperator::Min,
                    ::graphql_orm::graphql::orm::GraphqlAggregateOperator::Max,
                ] }
            } else {
                quote! { vec![::graphql_orm::graphql::orm::GraphqlAggregateOperator::Count] }
            };
            let aggregate_value_kind = if is_numeric {
                if is_float {
                    quote! { Some(::graphql_orm::graphql::orm::GraphqlAggregateValueKind::Floating) }
                } else if is_decimal {
                    quote! { Some(::graphql_orm::graphql::orm::GraphqlAggregateValueKind::Decimal) }
                } else {
                    quote! { Some(::graphql_orm::graphql::orm::GraphqlAggregateValueKind::Integral) }
                }
            } else if groupable && !is_boolean {
                quote! { Some(::graphql_orm::graphql::orm::GraphqlAggregateValueKind::Text) }
            } else {
                quote! { Some(::graphql_orm::graphql::orm::GraphqlAggregateValueKind::Count) }
            };
            let has_field_policy = field_meta.read_policy.is_some();
            let sortable = field_meta.sortable && field_meta.order;
            if field_meta.is_version {
                let mode = if field_meta.managed_version {
                    quote! { ::graphql_orm::graphql::orm::GraphqlRecordVersionMode::Managed }
                } else {
                    quote! { ::graphql_orm::graphql::orm::GraphqlRecordVersionMode::Cas }
                };
                semantic_record_version = quote! {
                    Some(::graphql_orm::graphql::orm::GraphqlRecordVersionMetadata {
                        field: ::std::string::ToString::to_string(#graphql_name),
                        mode: #mode,
                    })
                };
            }
            semantic_field_defs.push(quote! {
                ::graphql_orm::graphql::orm::__gom_semantic_field(
                ::graphql_orm::graphql::orm::GraphqlSemanticFieldMetadata {
                    field_name: ::std::string::ToString::to_string(#graphql_name),
                    description: ::std::string::ToString::to_string(#description),
                    type_ref: ::graphql_orm::graphql::orm::parse_graphql_type(
                        &<#field_type as ::graphql_orm::async_graphql::OutputType>::qualified_type_name(),
                    ).expect("macro-generated public GraphQL field type must be valid"),
                    selectable: true,
                    filter_operators: Vec::new(),
                    filter_input: #filter_input,
                    sortable: #sortable,
                    groupable: #groupable,
                    aggregate_operators: #aggregate_operators,
                    aggregate_value_kind: #aggregate_value_kind,
                    relationship: None,
                    classification: #classification_tokens,
                    export: #export_tokens,
                    has_field_policy: #has_field_policy,
                })
            });
        }
        if !field_meta.is_relation && !field_meta.skip_db {
            let read_policy = field_meta
                .read_policy
                .as_deref()
                .map(|policy| quote! { Some(#policy) })
                .unwrap_or_else(|| quote! { None });
            let write_policy = field_meta
                .write_policy
                .as_deref()
                .map(|policy| quote! { Some(#policy) })
                .unwrap_or_else(|| quote! { None });
            let sensitive = field_meta.sensitive;
            persisted_field_rust_names.push(rust_name.clone());
            repository_field_policy_defs.push(quote! {
                ::graphql_orm::graphql::orm::RepositoryFieldPolicyDef {
                    rust_name: #rust_name,
                    api_name: #graphql_name,
                    read_policy: #read_policy,
                    write_policy: #write_policy,
                    sensitive: #sensitive,
                }
            });
            if field_meta.graphql_readable() {
                let variant = syn::Ident::new(&rust_name.to_case(Case::Pascal), field.span());
                let column_name = field_meta
                    .db_column
                    .as_deref()
                    .unwrap_or(rust_name.as_str());
                let scalar_type = option_inner_type(field_type).unwrap_or(field_type);
                let aggregate_nullable = is_option_type(field_type);
                let type_name = type_path_last_ident(scalar_type)
                    .map(ToString::to_string)
                    .unwrap_or_default();
                let kind = if field_meta.decimal.is_some() {
                    let definition = decimal_definition_tokens(&field_meta);
                    quote! {
                        ::graphql_orm::graphql::orm::AggregateFieldKind::Decimal(
                            #definition
                        )
                    }
                } else if matches!(
                    type_name.as_str(),
                    "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64" | "usize"
                ) {
                    quote! { ::graphql_orm::graphql::orm::AggregateFieldKind::Integral }
                } else if matches!(type_name.as_str(), "f32" | "f64") {
                    quote! { ::graphql_orm::graphql::orm::AggregateFieldKind::Floating }
                } else if type_name == "bool" || field_meta.is_boolean_field {
                    quote! { ::graphql_orm::graphql::orm::AggregateFieldKind::Boolean }
                } else {
                    quote! { ::graphql_orm::graphql::orm::AggregateFieldKind::Text }
                };
                let groupable = !field_meta.is_json_field
                    && field_meta.spatial.is_none()
                    && !is_byte_vec_type(scalar_type);
                let operators = if field_meta.decimal.is_some()
                    || matches!(
                        type_name.as_str(),
                        "i8" | "i16"
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
                    ) {
                    quote! { &[
                        ::graphql_orm::graphql::orm::GraphqlAggregateOperator::Count,
                        ::graphql_orm::graphql::orm::GraphqlAggregateOperator::Min,
                        ::graphql_orm::graphql::orm::GraphqlAggregateOperator::Max,
                        ::graphql_orm::graphql::orm::GraphqlAggregateOperator::Sum,
                    ] }
                } else if groupable && type_name != "bool" && !field_meta.is_boolean_field {
                    quote! { &[
                        ::graphql_orm::graphql::orm::GraphqlAggregateOperator::Count,
                        ::graphql_orm::graphql::orm::GraphqlAggregateOperator::Min,
                        ::graphql_orm::graphql::orm::GraphqlAggregateOperator::Max,
                    ] }
                } else {
                    quote! { &[::graphql_orm::graphql::orm::GraphqlAggregateOperator::Count] }
                };
                aggregate_field_variants.push(quote! { #variant });
                aggregate_field_match_arms.push(quote! {
                    #aggregate_field_name::#variant => {
                        static FIELD: ::graphql_orm::graphql::orm::AggregateFieldRef =
                            ::graphql_orm::graphql::orm::AggregateFieldRef::generated(
                                #rust_name,
                                #graphql_name,
                                #column_name,
                                #kind,
                                #aggregate_nullable,
                                #groupable,
                                #operators,
                            );
                        &FIELD
                    }
                });
            }
        }
        let db_col = field_meta
            .db_column
            .clone()
            .unwrap_or_else(|| rust_name.clone());
        let db_col_sql = backend_render_identifier_path(backend, &db_col);
        let db_col_identity = backend_quote_identifier_path(backend, &db_col);
        let constraint_type = option_inner_type(field_type).unwrap_or(field_type);
        let constraint_type_name = quote! { #constraint_type }.to_string().replace(' ', "");
        let numeric_constraint_type = matches!(
            constraint_type_name.as_str(),
            "i8" | "i16"
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
        );
        if (field_meta.non_negative
            || field_meta.minimum.is_some()
            || field_meta.maximum.is_some()
            || field_meta.exclusive_minimum.is_some()
            || field_meta.exclusive_maximum.is_some())
            && !numeric_constraint_type
        {
            return Err(syn::Error::new(
                field.span(),
                "numeric constraints require a numeric scalar field",
            ));
        }
        if (field_meta.min_length.is_some() || field_meta.max_length.is_some())
            && !is_string_type(constraint_type)
            && !is_byte_vec_type(constraint_type)
        {
            return Err(syn::Error::new(
                field.span(),
                "length constraints require String or Vec<u8>",
            ));
        }
        if !field_meta.one_of.is_empty() && !is_string_type(constraint_type) {
            return Err(syn::Error::new(
                field.span(),
                "one_of currently requires String or Option<String>",
            ));
        }
        if let (Some(minimum), Some(maximum)) = (&field_meta.minimum, &field_meta.maximum) {
            if minimum
                .parse::<f64>()
                .ok()
                .zip(maximum.parse::<f64>().ok())
                .is_some_and(|(minimum, maximum)| minimum > maximum)
            {
                return Err(syn::Error::new(field.span(), "min must not exceed max"));
            }
        }
        if field_meta.minimum.is_some() && field_meta.exclusive_minimum.is_some() {
            return Err(syn::Error::new(
                field.span(),
                "min and min_exclusive cannot both be declared",
            ));
        }
        if field_meta.maximum.is_some() && field_meta.exclusive_maximum.is_some() {
            return Err(syn::Error::new(
                field.span(),
                "max and max_exclusive cannot both be declared",
            ));
        }
        let lower = field_meta
            .minimum
            .as_ref()
            .map(|value| (value, false))
            .or_else(|| {
                field_meta
                    .exclusive_minimum
                    .as_ref()
                    .map(|value| (value, true))
            });
        let upper = field_meta
            .maximum
            .as_ref()
            .map(|value| (value, false))
            .or_else(|| {
                field_meta
                    .exclusive_maximum
                    .as_ref()
                    .map(|value| (value, true))
            });
        if let (Some((lower, lower_exclusive)), Some((upper, upper_exclusive))) = (lower, upper)
            && lower
                .parse::<f64>()
                .ok()
                .zip(upper.parse::<f64>().ok())
                .is_some_and(|(lower, upper)| {
                    lower > upper || (lower == upper && (lower_exclusive || upper_exclusive))
                })
        {
            return Err(syn::Error::new(
                field.span(),
                "numeric lower bound must leave a non-empty range below the upper bound",
            ));
        }
        if field_meta
            .min_length
            .zip(field_meta.max_length)
            .is_some_and(|(minimum, maximum)| minimum > maximum)
        {
            return Err(syn::Error::new(
                field.span(),
                "min_length must not exceed max_length",
            ));
        }
        let constraint_prefix = format!(
            "graphql_orm_check_{}_{}",
            raw_table_name
                .chars()
                .map(|ch| if ch.is_ascii_alphanumeric() {
                    ch.to_ascii_lowercase()
                } else {
                    '_'
                })
                .collect::<String>(),
            db_col
                .chars()
                .map(|ch| if ch.is_ascii_alphanumeric() {
                    ch.to_ascii_lowercase()
                } else {
                    '_'
                })
                .collect::<String>(),
        );
        let mut push_constraint = |suffix: &str, expression: String| {
            let mut name = format!("{constraint_prefix}_{suffix}");
            if name.len() > 63 {
                let mut hash = 0xcbf29ce484222325u64;
                for byte in name.as_bytes() {
                    hash ^= u64::from(*byte);
                    hash = hash.wrapping_mul(0x100000001b3);
                }
                name = format!("{}_{}", &name[..46], format_args!("{hash:016x}"));
            }
            constraint_defs.push(quote! {
                ::graphql_orm::graphql::orm::CheckConstraintDef {
                    name: #name,
                    expression: #expression,
                }
            });
        };
        if field_meta.non_negative {
            push_constraint("non_negative", format!("{db_col_sql} >= 0"));
        }
        if let Some(minimum) = &field_meta.minimum {
            push_constraint("min", format!("{db_col_sql} >= {minimum}"));
        }
        if let Some(maximum) = &field_meta.maximum {
            push_constraint("max", format!("{db_col_sql} <= {maximum}"));
        }
        if let Some(minimum) = &field_meta.exclusive_minimum {
            push_constraint("min_exclusive", format!("{db_col_sql} > {minimum}"));
        }
        if let Some(maximum) = &field_meta.exclusive_maximum {
            push_constraint("max_exclusive", format!("{db_col_sql} < {maximum}"));
        }
        let length_function = if backend == BackendKind::Postgres && is_byte_vec_type(field_type) {
            "octet_length"
        } else {
            "length"
        };
        if let Some(minimum) = field_meta.min_length {
            push_constraint(
                "min_length",
                format!("{length_function}({db_col_sql}) >= {minimum}"),
            );
        }
        if let Some(maximum) = field_meta.max_length {
            push_constraint(
                "max_length",
                format!("{length_function}({db_col_sql}) <= {maximum}"),
            );
        }
        if !field_meta.one_of.is_empty() {
            let values = field_meta
                .one_of
                .iter()
                .map(|value| format!("'{}'", value.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(", ");
            push_constraint("one_of", format!("{db_col_sql} IN ({values})"));
        }
        for (constraint_name, operator, comparison_field) in [
            ("gte_field", ">=", &field_meta.gte_field),
            ("gt_field", ">", &field_meta.gt_field),
            ("lte_field", "<=", &field_meta.lte_field),
            ("lt_field", "<", &field_meta.lt_field),
        ] {
            let Some(other_rust_field) = comparison_field else {
                continue;
            };
            let other = parsed_fields
                .iter()
                .find(|parsed| {
                    parsed
                        .field
                        .ident
                        .as_ref()
                        .is_some_and(|ident| ident == other_rust_field)
                })
                .ok_or_else(|| {
                    syn::Error::new(
                        field.span(),
                        format!("{constraint_name} references unknown field `{other_rust_field}`"),
                    )
                })?;
            if other.meta.is_relation || other.meta.skip_db {
                return Err(syn::Error::new(
                    field.span(),
                    format!("{constraint_name} must reference a persisted scalar field"),
                ));
            }
            let other_type = option_inner_type(&other.field.ty).unwrap_or(&other.field.ty);
            if quote! { #constraint_type }.to_string().replace(' ', "")
                != quote! { #other_type }.to_string().replace(' ', "")
            {
                return Err(syn::Error::new(
                    field.span(),
                    format!("{constraint_name} requires fields with the same scalar type"),
                ));
            }
            let other_column = other
                .meta
                .db_column
                .clone()
                .unwrap_or_else(|| other_rust_field.clone());
            let other_column = backend_render_identifier_path(backend, &other_column);
            push_constraint(
                constraint_name,
                format!("{db_col_sql} {operator} {other_column}"),
            );
        }

        // Determine SQL type and nullability
        let is_nullable = is_option_type(field_type);
        let is_pk = field_meta.is_primary_key;
        let is_unique = field_meta.unique;
        let is_generated = field_meta
            .auto_generated
            .unwrap_or(field_meta.is_primary_key && rust_name == "id");
        let backup_policy =
            backup_policy_tokens(field_meta.backup_policy.as_deref(), field.span())?;
        let logical_type = backup_value_kind_tokens(field_type, &field_meta);
        let sql_type = rust_type_to_sql_type(backend, field_type, &field_meta);
        let decimal_tokens = field_meta
            .decimal
            .map(|_| {
                let definition = decimal_definition_tokens(&field_meta);
                quote! { Some(#definition) }
            })
            .unwrap_or_else(|| quote! { None });
        let spatial_tokens = if let Some(spatial) = &field_meta.spatial {
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
            let geometry_type = spatial_geometry_type_tokens(&spatial.geometry_type, field.span())?;
            let srid = spatial.srid;
            if spatial.index && backend == BackendKind::Postgres {
                let index_name = bounded_generated_identifier(format!(
                    "idx_{}_{}_spatial",
                    raw_table_name, db_col
                ));
                let col = syn::LitStr::new(&db_col, field.span());
                index_defs.push(quote! {
                    ::graphql_orm::graphql::orm::IndexDef::spatial_gist(#index_name, &[#col])
                });
            }
            if backend == BackendKind::Postgres {
                column_names.push(format!(
                    "ST_AsGeoJSON({}, 9, 8)::text AS {}",
                    db_col_sql, db_col_sql
                ));
            } else {
                column_names.push(db_col_sql.clone());
            }
            quote! {
                Some(::graphql_orm::graphql::orm::SpatialColumnDef::geometry(#geometry_type, #srid))
            }
        } else if field_meta.mssql_money {
            column_names.push(format!(
                "CAST({db_col_sql} AS DECIMAL(19,4)) AS {db_col_sql}"
            ));
            quote! { None }
        } else {
            column_names.push(db_col_sql.clone());
            quote! { None }
        };
        let search_tokens = if let Some(search) = &field_meta.search {
            if !is_string_type(field_type) {
                return Err(syn::Error::new(
                    field.span(),
                    "graphql_orm(searchable(...)) is only supported on String or Option<String> fields",
                ));
            }
            if !field_meta.graphql_readable() {
                return Err(syn::Error::new(
                    field.span(),
                    "unreadable fields cannot be full-text searchable",
                ));
            }
            if field_meta.read_policy.is_some() && search.policy.is_none() {
                return Err(syn::Error::new(
                    field.span(),
                    "searchable fields with read_policy require searchable(policy = \"...\")",
                ));
            }
            let weight = search_weight_tokens(&search.weight, field.span())?;
            let alias = option_lit_tokens(search.alias.as_deref(), field.span());
            let policy = option_lit_tokens(search.policy.as_deref(), field.span());
            search_field_defs.push(quote! {
                ::graphql_orm::graphql::orm::SearchFieldDef {
                    field_name: #rust_name,
                    column_name: #db_col,
                    weight: #weight,
                    alias: #alias,
                    policy: #policy,
                }
            });
            let chunk_value = if is_option_type(field_type) {
                quote! { self.#field_name.as_deref() }
            } else {
                quote! { Some(self.#field_name.as_str()) }
            };
            search_document_chunks.push(quote! {
                if let Some(value) = #chunk_value {
                    if !value.trim().is_empty() {
                        chunks.push(::graphql_orm::graphql::orm::SearchDocumentChunk {
                            source: ::graphql_orm::graphql::orm::SearchDocumentSource::Field {
                                field_name: #rust_name,
                            },
                            weight: #weight,
                            text: value.to_string(),
                        });
                    }
                }
            });
            quote! {
                Some(::graphql_orm::graphql::orm::SearchFieldDef {
                    field_name: #rust_name,
                    column_name: #db_col,
                    weight: #weight,
                    alias: #alias,
                    policy: #policy,
                })
            }
        } else {
            quote! { None }
        };
        if !field_meta.search_json.is_empty() {
            if !field_meta.is_json_field {
                return Err(syn::Error::new(
                    field.span(),
                    "graphql_orm(search_json(...)) requires a field marked #[graphql_orm(json)]",
                ));
            }
            if !field_meta.graphql_readable() {
                return Err(syn::Error::new(
                    field.span(),
                    "unreadable fields cannot use full-text search_json paths",
                ));
            }
            for search_json in &field_meta.search_json {
                if field_meta.read_policy.is_some() && search_json.policy.is_none() {
                    return Err(syn::Error::new(
                        field.span(),
                        "search_json fields with read_policy require search_json(policy = \"...\")",
                    ));
                }
                let weight = search_weight_tokens(&search_json.weight, field.span())?;
                let path = syn::LitStr::new(&search_json.path, field.span());
                let policy = option_lit_tokens(search_json.policy.as_deref(), field.span());
                search_json_path_defs.push(quote! {
                    ::graphql_orm::graphql::orm::SearchJsonPathDef {
                        field_name: #rust_name,
                        column_name: #db_col,
                        path: #path,
                        weight: #weight,
                        policy: #policy,
                    }
                });
                search_document_chunks.push(quote! {
                    if let Ok(value) = ::graphql_orm::serde_json::to_value(&self.#field_name) {
                        let text = ::graphql_orm::graphql::orm::search_json_path_text(&value, #path);
                        if !text.trim().is_empty() {
                            chunks.push(::graphql_orm::graphql::orm::SearchDocumentChunk {
                                source: ::graphql_orm::graphql::orm::SearchDocumentSource::JsonPath {
                                    field_name: #rust_name,
                                    path: #path,
                                },
                                weight: #weight,
                                text,
                            });
                        }
                    }
                });
            }
        }
        if field_meta.suppress_implicit_default
            && rust_name != "created_at"
            && rust_name != "updated_at"
            && !field_meta.host_managed_timestamp
        {
            return Err(syn::Error::new(
                field.span(),
                "default = false only suppresses the implicit created_at or updated_at default",
            ));
        }
        let explicit_default = match (field_meta.default.as_deref(), field_meta.decimal) {
            (Some(value), Some((precision, scale))) => Some(decimal_default_sql(
                backend,
                value,
                precision,
                scale,
                field.span(),
            )?),
            (Some(value), None) => Some(value.to_owned()),
            (None, _) => None,
        };
        let default_val = explicit_default.or_else(|| {
            if !field_meta.suppress_implicit_default
                && (rust_name == "created_at" || rust_name == "updated_at")
            {
                Some(
                    if is_string_type(field_type) {
                        backend_current_timestamp_expr(backend, field_meta.is_date_field)
                    } else {
                        backend_current_epoch_expr(backend)
                    }
                    .to_string(),
                )
            } else {
                None
            }
        });

        // Build column definition
        let default_expr = match default_val {
            Some(d) => {
                let lit = syn::LitStr::new(&d, proc_macro2::Span::call_site());
                quote! { Some(#lit) }
            }
            None => quote! { None },
        };
        let is_readable = scalar_output_is_readable(
            field,
            &field_meta,
            !schema_only
                && !legacy_graphql_complex
                && !has_repository_entity_attribute(&input.attrs),
            &input.attrs,
        );
        let is_filterable = field_meta.filter && field_meta.filterable.is_some();
        let is_sortable = field_meta.sortable && field_meta.order;
        let is_date_time = field_meta.is_date_field;

        column_defs.push(quote! {
            ::graphql_orm::graphql::orm::ColumnDef {
                name: #db_col,
                rust_name: #rust_name,
                api_name: #graphql_name,
                sql_type: #sql_type,
                decimal: #decimal_tokens,
                spatial: #spatial_tokens,
                search: #search_tokens,
                logical_type: #logical_type,
                nullable: #is_nullable,
                is_primary_key: #is_pk,
                is_unique: #is_unique,
                is_generated: #is_generated,
                is_readable: #is_readable,
                is_filterable: #is_filterable,
                is_sortable: #is_sortable,
                is_date_time: #is_date_time,
                backup_policy: #backup_policy,
                default: #default_expr,
                references: None,
            }
        });

        if field_meta.is_primary_key {
            primary_key_cols.push(db_col_identity.clone());
        }

        // Generate WhereInput field for filterable fields
        if field_meta.filter {
            if let Some(ref filter_type) = field_meta.filterable {
                let filter_field_name =
                    rust_ident_from_graphql_name(&graphql_name, field_name.span());
                filter_referenced_field_checks.push(quote! {
                    if self.#filter_field_name.is_some() {
                        fields.push(#graphql_name);
                    }
                });
                let (
                    input_field,
                    sql_gen,
                    entity_match_gen,
                    is_empty_check,
                    contains_spatial_check,
                ) = generate_filter_field(
                    backend,
                    struct_name,
                    field_name,
                    field_type,
                    &graphql_name,
                    &db_col_sql,
                    filter_type,
                    &field_meta,
                )?;
                where_input_fields.push(input_field);
                filter_to_sql.push(sql_gen);
                filter_to_entity_match.push(entity_match_gen);
                filter_is_empty_checks.push(is_empty_check);
                filter_contains_spatial_checks.push(contains_spatial_check);
                if filter_type == "date" || filter_type == "int64" {
                    filter_validation_checks.push(quote! {
                        if let Some(filter) = &self.#filter_field_name {
                            filter.validate()?;
                        }
                    });
                }
            }
        }

        // Generate OrderByInput field for sortable fields
        if field_meta.sortable && field_meta.order {
            let order_field_name = rust_ident_from_graphql_name(&graphql_name, field_name.span());
            if !order_by_graphql_names.insert(graphql_name.clone()) {
                return Err(syn::Error::new_spanned(
                    field,
                    format!("duplicate generated ordering field `{graphql_name}`"),
                ));
            }
            if !order_by_rust_names.insert(order_field_name.to_string()) {
                return Err(syn::Error::new_spanned(
                    field,
                    format!(
                        "ordering field `{graphql_name}` conflicts with another generated Rust order-input field"
                    ),
                ));
            }
            sortable_columns.push((order_field_name.clone(), db_col_sql.clone()));
            order_referenced_field_checks.push(quote! {
                if self.#order_field_name.is_some() {
                    fields.push(#graphql_name);
                }
            });
            order_by_fields.push(quote! {
                #[graphql(name = #graphql_name)]
                pub #order_field_name: Option<::graphql_orm::graphql::orm::OrderDirection>,
            });
        }

        if field_meta.graphql_readable() {
            let getter_name = field_name;
            let description = field_meta
                .description
                .as_deref()
                .expect("field descriptions are resolved during parsing");
            let subscribe_check = if field_meta.subscribe {
                quote! {}
            } else {
                quote! {
                    if ctx.query_env.operation.node.ty
                        == ::graphql_orm::async_graphql::parser::types::OperationType::Subscription
                    {
                        return Err(::graphql_orm::async_graphql::Error::new(format!(
                            "Field {}.{} is not available in subscriptions",
                            #entity_name_lit,
                            #graphql_name,
                        )));
                    }
                }
            };
            let policy_record =
                if identity_only && legacy_graphql_complex && entity_meta.compose_complex_object {
                    quote! { self.0 }
                } else {
                    quote! { self }
                };
            let policy_check = if let Some(policy_key) = &field_meta.read_policy {
                quote! {
                    db.ensure_readable_field(
                        ctx,
                        #entity_name_lit,
                        #graphql_name,
                        Some(#policy_key),
                        Some(#policy_record as &(dyn ::std::any::Any + Send + Sync)),
                    ).await
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                }
            } else {
                quote! {}
            };
            let (return_type, return_expr) = if field_meta.spatial.is_some() {
                if is_option_type(field_type) {
                    (
                        quote! { Option<::graphql_orm::async_graphql::Json<::graphql_orm::serde_json::Value>> },
                        quote! { Ok(self.#field_name.clone().map(::graphql_orm::async_graphql::Json)) },
                    )
                } else {
                    (
                        quote! { ::graphql_orm::async_graphql::Json<::graphql_orm::serde_json::Value> },
                        quote! { Ok(::graphql_orm::async_graphql::Json(self.#field_name.clone())) },
                    )
                }
            } else {
                (
                    quote! { #field_type },
                    quote! { Ok(self.#field_name.clone()) },
                )
            };
            object_field_methods.push(quote! {
                #[doc = #description]
                #[graphql(name = #graphql_name)]
                async fn #getter_name(
                    &self,
                    ctx: &::graphql_orm::async_graphql::Context<'_>,
                ) -> ::graphql_orm::async_graphql::Result<#return_type> {
                    let db = ctx.data_unchecked::<::graphql_orm::db::Database<#backend_marker>>();
                    #subscribe_check
                    #policy_check
                    #return_expr
                }
            });
            if managed_version_api_name.as_deref() == Some(graphql_name.as_str()) {
                let record_version_description = format!(
                    "Managed record version of this {struct_name}: the value of `{graphql_name}`, advanced by every generated write."
                );
                let method = quote! {
                    #[doc = #record_version_description]
                    #[graphql(name = "RecordVersion")]
                    async fn __gom_record_version(
                        &self,
                        ctx: &::graphql_orm::async_graphql::Context<'_>,
                    ) -> ::graphql_orm::async_graphql::Result<#return_type> {
                        let db = ctx.data_unchecked::<::graphql_orm::db::Database<#backend_marker>>();
                        #subscribe_check
                        #policy_check
                        #return_expr
                    }
                };
                object_field_methods.push(method.clone());
                identity_field_methods.push(method);
                identity_field_names.push(RECORD_VERSION_FIELD);
            }
        }

        // Generate FromSqlRow field assignment
        let row_assignment =
            generate_row_field_assignment(backend, field_name, field_type, &db_col, &field_meta)?;
        from_row_fields.push(row_assignment);
    }

    for order_expression in &entity_meta.order_expressions {
        if !order_by_graphql_names.insert(order_expression.name.clone()) {
            return Err(syn::Error::new(
                struct_name.span(),
                format!(
                    "duplicate generated ordering field `{}`",
                    order_expression.name
                ),
            ));
        }
        let graphql_name = &order_expression.name;
        let order_field_name = rust_ident_from_graphql_name(graphql_name, struct_name.span());
        if !order_by_rust_names.insert(order_field_name.to_string()) {
            return Err(syn::Error::new(
                struct_name.span(),
                format!(
                    "ordering field `{graphql_name}` conflicts with another generated Rust order-input field"
                ),
            ));
        }
        let expression = format!("({})", order_expression.expression);
        sortable_columns.push((order_field_name.clone(), expression));
        if let Some(provider) = &order_expression.parameter_provider {
            order_expression_context_checks.push(quote! { self.#order_field_name.is_some() });
            let parameter_names = order_expression
                .parameter_names
                .iter()
                .map(|name| syn::LitStr::new(name, struct_name.span()))
                .collect::<Vec<_>>();
            order_expression_parameter_arms.push(quote! {
                if self.#order_field_name.is_some() {
                    let __graphql_orm_parameters = #provider(ctx)?;
                    #(
                        values.push(__graphql_orm_parameters.require(#parameter_names)?);
                    )*
                }
            });
        }
        order_by_fields.push(quote! {
            #[graphql(name = #graphql_name)]
            pub #order_field_name: Option<::graphql_orm::graphql::orm::OrderDirection>,
        });
    }

    if primary_key_cols.is_empty() {
        // Without `#[primary_key]`, an `id` field is the key; use its physical
        // column so a `db_column` rename reaches every key consumer.
        let implicit_key =
            implicit_id_db_column(&parsed_fields).unwrap_or_else(|| "id".to_string());
        primary_key_cols.push(backend_quote_identifier_path(backend, &implicit_key));
    }
    // An omitted default sort orders by every primary-key column, in
    // declaration order. A key that is not named `id` must not fall back to a
    // literal `id` column (#404).
    let default_sort = match entity_meta.default_sort.as_deref() {
        Some(raw) if backend == BackendKind::Mssql && !raw.chars().any(char::is_whitespace) => {
            backend_quote_identifier_path(backend, raw)
        }
        Some(raw) => raw.to_string(),
        None => primary_key_default_sort(backend, &primary_key_cols),
    };
    let primary_key = primary_key_cols.first().map(String::as_str).unwrap_or("id");
    let primary_key_literals: Vec<syn::LitStr> = primary_key_cols
        .iter()
        .map(|column| syn::LitStr::new(column, struct_name.span()))
        .collect();
    let schema_policy_const = schema_policy_tokens(entity_meta.schema_policy.as_deref());
    let columns_array: Vec<&str> = column_names.iter().map(|s| s.as_str()).collect();
    let has_search = !search_field_defs.is_empty()
        || !search_json_path_defs.is_empty()
        || !search_relation_defs.is_empty();
    let search_config = entity_meta.search.clone().unwrap_or_default();
    let search_enabled = search_config.index;
    let search_language = search_config.language;
    let search_tokenizer = search_config.tokenizer;
    let search_min_token_len = search_config.min_token_len;
    let search_fallback_enabled = search_config.fallback_enabled;
    let search_index_name = bounded_generated_identifier(format!(
        "idx_gom_search_{}_vector",
        raw_table_name.replace('.', "_")
    ));
    let search_pk = parsed_fields
        .iter()
        .find(|parsed| {
            parsed.meta.is_primary_key && !parsed.meta.is_relation && !parsed.meta.skip_db
        })
        .or_else(|| {
            parsed_fields.iter().find(|parsed| {
                parsed
                    .field
                    .ident
                    .as_ref()
                    .is_some_and(|ident| ident == "id")
                    && !parsed.meta.is_relation
                    && !parsed.meta.skip_db
            })
        });
    let search_pk_field = search_pk
        .and_then(|parsed| parsed.field.ident.clone())
        .unwrap_or_else(|| syn::Ident::new("id", struct_name.span()));
    let search_pk_is_bytes = search_pk.is_some_and(|parsed| is_byte_vec_type(&parsed.field.ty));
    let search_strategy = match backend {
        BackendKind::Postgres => {
            quote! { ::graphql_orm::graphql::orm::SearchIndexStrategy::PostgresTsvector }
        }
        BackendKind::Sqlite => {
            quote! { ::graphql_orm::graphql::orm::SearchIndexStrategy::SqliteFts5 }
        }
        BackendKind::Mssql => {
            quote! { ::graphql_orm::graphql::orm::SearchIndexStrategy::MssqlFullText }
        }
    };
    let search_schema_impl = if has_search {
        quote! {
            impl ::graphql_orm::graphql::orm::DatabaseSearchSchema for #struct_name {
                fn search_index() -> Option<&'static ::graphql_orm::graphql::orm::SearchIndexDef> {
                    static FIELDS: &[::graphql_orm::graphql::orm::SearchFieldDef] = &[
                        #(#search_field_defs),*
                    ];
                    static JSON_PATHS: &[::graphql_orm::graphql::orm::SearchJsonPathDef] = &[
                        #(#search_json_path_defs),*
                    ];
                    static RELATIONS: &[::graphql_orm::graphql::orm::SearchRelationFieldDef] = &[
                        #(#search_relation_defs),*
                    ];
                    static INDEX: ::graphql_orm::graphql::orm::SearchIndexDef =
                        ::graphql_orm::graphql::orm::SearchIndexDef {
                            entity_name: #entity_name_lit,
                            table_name: #table_name,
                            primary_key: #primary_key,
                            index_name: #search_index_name,
                            strategy: #search_strategy,
                            enabled: #search_enabled,
                            language: #search_language,
                            tokenizer: #search_tokenizer,
                            min_token_len: #search_min_token_len,
                            fallback_enabled: #search_fallback_enabled,
                            fields: FIELDS,
                            json_paths: JSON_PATHS,
                            relations: RELATIONS,
                        };
                    Some(&INDEX)
                }
            }
        }
    } else {
        quote! {
            impl ::graphql_orm::graphql::orm::DatabaseSearchSchema for #struct_name {}
        }
    };
    let searchable_entity_impl = if has_search {
        let search_key = if search_pk_is_bytes {
            quote! { ::graphql_orm::graphql::orm::binary_key_id(&self.#search_pk_field) }
        } else {
            quote! { self.#search_pk_field.to_string() }
        };
        let search_key_json_fallback = search_key.clone();
        quote! {
            impl ::graphql_orm::graphql::orm::SearchableEntity for #struct_name {
                fn search_key(&self) -> String {
                    #search_key
                }

                fn search_key_json(&self) -> ::graphql_orm::serde_json::Value {
                    ::graphql_orm::serde_json::to_value(&self.#search_pk_field)
                        .unwrap_or_else(|_| ::graphql_orm::serde_json::Value::String(#search_key_json_fallback))
                }

                fn search_document(&self) -> ::graphql_orm::graphql::orm::SearchDocument {
                    let mut chunks = Vec::new();
                    #(#search_document_chunks)*
                    ::graphql_orm::graphql::orm::SearchDocument {
                        entity_pk: self.search_key(),
                        entity_pk_json: self.search_key_json(),
                        chunks,
                    }
                }

                fn authorize_search_relation_targets(
                    ctx: &::graphql_orm::async_graphql::Context<'_>,
                ) -> ::graphql_orm::async_graphql::Result<()> {
                    let _ = ctx;
                    #(#search_relation_traversal_checks)*
                    Ok(())
                }

                fn search_relation_source_policies(
                ) -> Option<Vec<::graphql_orm::graphql::orm::SearchSourcePolicy>> {
                    #[allow(unused_mut)]
                    let mut sources = Vec::new();
                    #(#search_relation_policy_tokens)*
                    Some(sources)
                }
            }
        }
    } else {
        quote! {}
    };

    // Generate type names (as strings for #[graphql(name = "...")] and as idents for struct names)
    let where_input_name_str = format!("{}WhereInput", struct_name);
    let order_by_name_str = format!("{}OrderByInput", struct_name);
    let where_input_name = syn::Ident::new(&where_input_name_str, struct_name.span());
    let order_by_name = syn::Ident::new(&order_by_name_str, struct_name.span());
    let struct_name_str = struct_name.to_string();
    if entity_meta.aggregate && aggregate_field_variants.is_empty() {
        return Err(syn::Error::new(
            struct_name.span(),
            "aggregate = true requires at least one public persisted readable field",
        ));
    }
    let aggregate_field_name_str = aggregate_field_name.to_string();
    let aggregate_graphql_derive = (!has_repository_entity_attribute(&input.attrs))
        .then(|| quote! { ::graphql_orm::async_graphql::Enum, });
    let aggregate_graphql_attribute = (!has_repository_entity_attribute(&input.attrs)).then(|| {
        quote! { #[graphql(name = #aggregate_field_name_str, rename_items = #field_case_rule)] }
    });
    let aggregate_field_definition = if aggregate_field_variants.is_empty() {
        quote! {}
    } else {
        quote! {
        /// Closed public fields accepted by typed aggregate queries for this entity.
        #[derive(
            #aggregate_graphql_derive
            Clone,
            Copy,
            Debug,
            Eq,
            Hash,
            PartialEq,
        )]
        #aggregate_graphql_attribute
        #[allow(clippy::enum_variant_names)]
        pub enum #aggregate_field_name {
            #(#aggregate_field_variants),*
        }

        impl ::graphql_orm::graphql::orm::TypedAggregateField<#struct_name>
            for #aggregate_field_name
        {
            fn entity_type_id() -> Option<::std::any::TypeId> {
                Some(::std::any::TypeId::of::<#struct_name>())
            }
            fn aggregate_field(self) -> &'static ::graphql_orm::graphql::orm::AggregateFieldRef {
                match self {
                    #(#aggregate_field_match_arms),*
                }
            }
        }

        impl #struct_name {
            /// Begins a policy-aware typed grouped aggregate query.
            pub fn aggregate<'a>(
                db: &'a ::graphql_orm::db::Database<#backend_marker>,
            ) -> ::graphql_orm::graphql::orm::GroupedAggregateQuery<
                'a,
                Self,
                #where_input_name,
                #aggregate_field_name,
                #backend_marker,
            > {
                ::graphql_orm::graphql::orm::GroupedAggregateQuery::new(db)
            }
        }
        }
    };
    let projection_definitions = generate_projection_definitions(
        backend,
        struct_name,
        &entity_meta,
        &parsed_fields,
        &where_input_name,
        &order_by_name,
        &backend_marker,
        &row_type,
        &helper_import,
        &input.vis,
    )?;

    // Generate order_by to_sql_order implementation
    let order_by_match_arms: Vec<_> = sortable_columns
        .iter()
        .map(|(field_ident, col)| {
            quote! {
                if let Some(dir) = &self.#field_ident {
                    parts.push(format!("{} {}", #col, dir.to_sql()));
                }
            }
        })
        .collect();

    if !schema_only
        && let Ok(primary_key_fields) = crate::operations::collect_primary_key_fields(
            struct_name,
            fields,
            graphql_rename_fields,
            serde_rename_all,
        )
        && let Ok(Some(parts)) = crate::federation::resolve_node_id_parts(
            struct_name,
            &entity_meta,
            &primary_key_fields,
            backend,
        )
    {
        let method = crate::federation::node_id_field_tokens(&struct_name_str, &parts);
        object_field_methods.push(method.clone());
        identity_field_methods.push(method);
        identity_field_names.push("NodeId");
        subscription_field_policy_arms.push(quote! {
            "NodeId" => Some(("__gom_node_id", true, None)),
        });
    }

    if let Some(names) = identity_names {
        names.extend(identity_field_names);
        return Ok(quote! { #(#identity_field_methods)* });
    }

    let managed_version_field_tokens = match &managed_version_api_name {
        Some(name) if !schema_only => quote! { Some(#name) },
        _ => quote! { None },
    };
    // Reported beside the expansion so one misnamed field is the only error.
    let identity_field_error = crate::composed_identity::check_struct_fields(
        &parsed_fields,
        externally_owned_object,
        graphql_rename_fields,
        serde_rename_all,
        entity_meta.node_id.is_some(),
        managed_version_api_name.is_some(),
    )
    .err()
    .map(|error| error.to_compile_error());
    // `GraphQLSchemaEntity` has no generated object, but a relation-less composed
    // entity still needs its composed object, which `GraphQLRelations` leaves to
    // the entity derive.
    let schema_entity_composed = if schema_only_override
        && !entity_meta.schema_only
        && crate::relations::entity_owns_composed_object(
            legacy_graphql_complex,
            &entity_meta,
            &parsed_fields,
        ) {
        let (identity_fields, identity_names) = generate_complex_identity_fields(input)?;
        if identity_names.is_empty() {
            quote! {}
        } else {
            let composed =
                crate::relations::composed_object_tokens(input, identity_fields, &identity_names);
            quote! {
                #composed

                impl #struct_name {
                    #[doc(hidden)]
                    pub fn __gom_generated_relation_fields() -> &'static [&'static str] {
                        &[]
                    }
                }
            }
        }
    } else {
        quote! {}
    };
    // A composed object reaches the schema only through `#[graphql_complex_object]`,
    // whichever derive emits it, so a declared key needs that impl too.
    let composed_mounted = if !schema_only
        && legacy_graphql_complex
        && entity_meta.compose_complex_object
        && entity_meta.node_id.is_some()
    {
        quote! {
            const _: () = {
                const fn assert_mounted<
                    T: ::graphql_orm::graphql::orm::GeneratedComplexObjectMounted,
                >() {}
                assert_mounted::<#struct_name>();
            };
        }
    } else {
        quote! {}
    };
    let object_impl = if !schema_only
        && !identity_field_methods.is_empty()
        && crate::relations::entity_owns_composed_object(
            legacy_graphql_complex,
            &entity_meta,
            &parsed_fields,
        ) {
        // No relation resolvers exist, so mount the identity fields here for
        // `#[graphql_complex_object]` to flatten beside the handwritten ones.
        let (identity_fields, identity_names) = generate_complex_identity_fields(input)?;
        let composed =
            crate::relations::composed_object_tokens(input, identity_fields, &identity_names);
        quote! {
            #composed
            #composed_mounted

            impl #struct_name {
                #[doc(hidden)]
                pub fn __gom_generated_relation_fields() -> &'static [&'static str] {
                    &[]
                }
            }
        }
    } else if legacy_graphql_complex && entity_meta.node_id.is_some() {
        // The relation derive owns the ComplexObject implementation. Require it
        // rather than accepting a resolvable key whose field was never emitted.
        quote! {
            const _: () = {
                const fn assert_mounted<
                    T: ::graphql_orm::graphql::orm::GeneratedComplexIdentityFields,
                >() {}
                assert_mounted::<#struct_name>();
            };
            #composed_mounted
        }
    } else if schema_only || legacy_graphql_complex {
        quote! {}
    } else {
        quote! {
            #[doc = #entity_description]
            #[::graphql_orm::async_graphql::Object]
            impl #struct_name {
                #(#object_field_methods)*
            }
        }
    };

    let persisted_field_names_impl = quote! {
        impl #struct_name {
            /// Rust names of the entity's persisted fields, for generated
            /// compile-time checks in other derives.
            #[doc(hidden)]
            pub const __GOM_PERSISTED_FIELD_NAMES: &'static [&'static str] =
                &[#(#persisted_field_rust_names),*];
        }
    };
    if schema_only {
        return Ok(quote! {
            #schema_entity_composed
            #identity_field_error
            #relation_readability_impl
            #(#relation_target_checks)*
        #persisted_field_names_impl
        #(#search_relation_field_checks)*
            #rls_impl
            #policy_object_witness
            #search_schema_impl
            #searchable_entity_impl

            impl ::graphql_orm::graphql::orm::DatabaseEntity for #struct_name {
                const TABLE_NAME: &'static str = #table_name;
                const PLURAL_NAME: &'static str = #plural_name;
                const PRIMARY_KEY: &'static str = #primary_key;
                const PRIMARY_KEYS: &'static [&'static str] = &[#(#primary_key_literals),*];
                const SCHEMA_POLICY: Option<::graphql_orm::graphql::orm::SchemaPolicy> = #schema_policy_const;
                const DEFAULT_SORT: &'static str = #default_sort;

                fn column_names() -> &'static [&'static str] {
                    &[#(#columns_array),*]
                }
            }

            impl ::graphql_orm::graphql::orm::DatabaseSchema for #struct_name {
                fn columns() -> &'static [::graphql_orm::graphql::orm::ColumnDef] {
                    static COLUMNS: &[::graphql_orm::graphql::orm::ColumnDef] = &[
                        #(#column_defs),*
                    ];
                    COLUMNS
                }

                fn indexes() -> &'static [::graphql_orm::graphql::orm::IndexDef] {
                    static INDEXES: &[::graphql_orm::graphql::orm::IndexDef] = &[
                        #(#index_defs),*
                    ];
                    INDEXES
                }

                fn composite_unique_indexes() -> &'static [&'static [&'static str]] {
                    static UNIQUE_INDEXES: &[&[&str]] = &[
                        #(#composite_unique_indexes),*
                    ];
                    UNIQUE_INDEXES
                }

                fn check_constraints() -> &'static [::graphql_orm::graphql::orm::CheckConstraintDef] {
                    static CONSTRAINTS: &[::graphql_orm::graphql::orm::CheckConstraintDef] = &[
                        #(#constraint_defs),*
                    ];
                    CONSTRAINTS
                }
            }

            impl ::graphql_orm::graphql::orm::EntityRelations for #struct_name {
                fn relation_metadata() -> &'static [::graphql_orm::graphql::orm::RelationMetadata] {
                    // Inherent methods win over this local blanket fallback.
                    // No witness means no generated getter ownership.
                    #[allow(dead_code)]
                    trait OutputFallback {
                        fn __gom_exposed_relation_fields() -> &'static [&'static str] { &[] }
                    }
                    impl<T> OutputFallback for T {}
                    static RELATIONS: ::std::sync::OnceLock<::std::boxed::Box<[::graphql_orm::graphql::orm::RelationMetadata]>> =
                        ::std::sync::OnceLock::new();
                    RELATIONS.get_or_init(|| {
                        let _generated_output = #struct_name::__gom_exposed_relation_fields();
                        vec![#(#relation_metadata_defs),*].into_boxed_slice()
                    })
                }
            }

            impl ::graphql_orm::graphql::orm::Entity for #struct_name {
                fn entity_name() -> &'static str {
                    #struct_name_str
                }

                fn metadata() -> &'static ::graphql_orm::graphql::orm::EntityMetadata {
                    static METADATA: ::std::sync::OnceLock<::graphql_orm::graphql::orm::EntityMetadata> =
                        ::std::sync::OnceLock::new();
                    METADATA.get_or_init(|| {
                        ::graphql_orm::graphql::orm::EntityMetadata::from_schema_with_retention::<Self>(
                            #struct_name_str,
                            #backup_enabled,
                            #backup_export_order,
                            #backup_restore_order,
                            #read_policy,
                            #write_policy,
                            #append_only,
                            #retention_policy,
                        )
                    })
                }

                fn graphql_semantic_metadata() -> Option<&'static ::graphql_orm::graphql::orm::GraphqlEntitySemanticMetadata> {
                    static METADATA: ::std::sync::OnceLock<
                        ::graphql_orm::graphql::orm::GraphqlEntitySemanticMetadata
                    > = ::std::sync::OnceLock::new();
                    Some(METADATA.get_or_init(|| {
                        ::graphql_orm::graphql::orm::GraphqlEntitySemanticMetadata {
                            generated_input_members: vec![#(#hidden_input_member_defs),*],
                            entity_name: ::std::string::ToString::to_string(#struct_name_str),
                            description: ::std::string::ToString::to_string(#entity_description),
                            default_classification: #entity_classification_tokens,
                            fields: vec![#(#semantic_field_defs),*].into_boxed_slice(),
                            label_fields: ::std::vec::Vec::new(),
                            record_version: None,
                        }
                    }))
                }

                fn subscription_field_policy(field: &str) -> Option<(&'static str, bool, Option<&'static str>)> {
                    match field { #(#subscription_field_policy_arms)* _ => None }
                }

                fn repository_field_policies() -> &'static [::graphql_orm::graphql::orm::RepositoryFieldPolicyDef] {
                    static FIELDS: &[::graphql_orm::graphql::orm::RepositoryFieldPolicyDef] = &[
                        #(#repository_field_policy_defs),*
                    ];
                    FIELDS
                }
            }

            impl ::graphql_orm::graphql::orm::GraphqlSemanticResultTypeMetadata for #struct_name {
                fn graphql_semantic_result_type(
                ) -> &'static ::graphql_orm::graphql::orm::GraphqlEntitySemanticMetadata {
                    <Self as ::graphql_orm::graphql::orm::Entity>
                        ::graphql_semantic_metadata()
                        .expect("GraphQLEntity must emit semantic metadata")
                }
            }
        });
    }

    let keyset_filter_identity_impl = if has_repository_entity_attribute(&input.attrs) {
        quote! {}
    } else {
        quote! {
            fn keyset_filter_identity(&self) -> Option<String> {
                if ::graphql_orm::graphql::orm::DatabaseFilter::is_empty(self) { return None; }
                Some(::graphql_orm::async_graphql::InputType::to_value(self).to_string())
            }
        }
    };
    let related_storage = if related_prepare.is_empty() {
        quote! {}
    } else {
        quote! {
            /// Request-authorized relation predicates. Internal to generated resolvers.
            #[doc(hidden)]
            #[graphql(skip)]
            pub __gom_related: ::std::collections::BTreeMap<String, (Vec<String>, Vec<::graphql_orm::graphql::orm::SqlValue>)>,
        }
    };
    let related_clear = if related_prepare.is_empty() {
        quote! {}
    } else {
        quote! { self.__gom_related.clear(); }
    };
    let relation_order_storage = if relation_order_prepare.is_empty() {
        quote! {}
    } else {
        quote! {
            /// Request-authorized relation count expressions. Internal to generated resolvers.
            #[doc(hidden)]
            #[graphql(skip)]
            pub __gom_relation_counts: ::std::collections::BTreeMap<String, (String, Vec<::graphql_orm::graphql::orm::SqlValue>)>,
        }
    };
    let relation_order_clear = if relation_order_prepare.is_empty() {
        quote! {}
    } else {
        quote! { self.__gom_relation_counts.clear(); }
    };
    // Repository-only entities strip the GraphQL derives from these helpers.
    // Keep their empty Rust order marker valid. GraphQL entities, however, must
    // supply a nonempty input: operation and cross-entity relation signatures
    // both name this type. Diagnose the owning declaration rather than letting
    // async-graphql's empty-input error cascade through those signatures.
    let empty_order_diagnostic = if order_by_fields.is_empty()
        && !has_repository_entity_attribute(&input.attrs)
    {
        let error = syn::Error::new(
            struct_name.span(),
            format!(
                "GraphQLEntity `{struct_name}` has no orderable fields, but generated query and relation APIs require `{order_by_name_str}`; add #[sortable] to a persisted field (for example the primary key)"
            ),
        ).to_compile_error();
        // This recovery-only field lets dependent expansions type-check, so the
        // macro-owned diagnostic remains the only error. Compilation always
        // fails above; this is never a supported or observable GraphQL input.
        order_by_fields.push(quote! {
            #[doc(hidden)]
            pub __gom_invalid_empty_order: Option<bool>,
        });
        error
    } else {
        quote! {}
    };
    Ok(quote! {
        #empty_order_diagnostic
        #relation_readability_impl
        #(#relation_target_checks)*
        #persisted_field_names_impl
        #(#search_relation_field_checks)*
        #rls_impl
        #policy_object_witness
        #search_schema_impl
        #searchable_entity_impl
        #projection_definitions
        #aggregate_field_definition

        // WhereInput for filtering
        #[derive(::graphql_orm::async_graphql::InputObject, Default, Clone, Debug)]
        #[graphql(name = #where_input_name_str, rename_fields = #field_case_rule)]
        pub struct #where_input_name {
            #(#where_input_fields)*
            #related_storage

            /// Logical AND of conditions
            pub and: Option<Vec<#where_input_name>>,

            /// Logical OR of conditions
            pub or: Option<Vec<#where_input_name>>,

            /// Logical NOT of condition
            pub not: Option<Box<#where_input_name>>,
        }

        // OrderByInput for sorting
        #[derive(::graphql_orm::async_graphql::InputObject, Default, Clone, Debug)]
        #[graphql(name = #order_by_name_str, rename_fields = #field_case_rule)]
        pub struct #order_by_name {
            #(#order_by_fields)*
            #relation_order_storage
        }

        impl #order_by_name {
            /// Authorize relation count ordering under the current request.
            /// Each count covers only target rows the request may read; callback
            /// or residual target policies fail closed.
            #[doc(hidden)]
            pub async fn __gom_prepare_read(
                &mut self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
            ) -> ::graphql_orm::async_graphql::Result<()> {
                let _ = ctx;
                #relation_order_clear
                #(#relation_order_prepare)*
                Ok(())
            }
        }

        #object_impl
        #identity_field_error

        impl ::graphql_orm::graphql::orm::DatabaseOrderBy for #order_by_name {
            fn to_sql_order(&self) -> Option<String> {
                let mut parts: Vec<String> = Vec::new();
                #(#order_by_match_arms)*
                #(#relation_order_match_arms)*
                if parts.is_empty() {
                    None
                } else {
                    Some(parts.join(", "))
                }
            }

            fn requires_context(&self) -> bool {
                false #(|| #order_expression_context_checks)* #(|| #relation_order_present)*
            }

            fn validate_order(&self) -> ::graphql_orm::Result<()> {
                #(#relation_order_validate)*
                Ok(())
            }

            fn referenced_fields(&self) -> Vec<&'static str> {
                #[allow(unused_mut)]
                let mut fields = Vec::new();
                #(#order_referenced_field_checks)*
                fields
            }

            fn to_sort_expression(&self) -> Option<::graphql_orm::graphql::orm::SortExpression> {
                let clause = self.to_sql_order()?;
                #[allow(unused_mut)]
                let mut values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();
                #(#relation_order_value_arms)*
                Some(::graphql_orm::graphql::orm::SortExpression { clause, values })
            }

            fn to_sort_expression_with_context(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
            ) -> ::graphql_orm::async_graphql::Result<
                Option<::graphql_orm::graphql::orm::SortExpression>
            > {
                let Some(clause) = self.to_sql_order() else {
                    return Ok(None);
                };
                ::graphql_orm::graphql::orm::DatabaseOrderBy::validate_order(self)
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                #[allow(unused_mut)]
                let mut values = Vec::new();
                #(#order_expression_parameter_arms)*
                #(#relation_order_value_arms)*
                Ok(Some(::graphql_orm::graphql::orm::SortExpression {
                    clause,
                    values,
                }))
            }
        }

        impl ::graphql_orm::graphql::orm::ReadVisibilityFilter for #where_input_name {
            fn entity_type_id() -> ::std::any::TypeId { ::std::any::TypeId::of::<#struct_name>() }
            fn backend() -> ::graphql_orm::graphql::orm::DatabaseBackend { <#backend_marker as ::graphql_orm::graphql::orm::OrmBackend>::DIALECT }
        }

        impl ::graphql_orm::graphql::orm::DatabaseFilter for #where_input_name {
            fn validate(&self) -> ::graphql_orm::Result<()> {
                #(#related_validate)*
                #(#filter_validation_checks)*
                if let Some(filters) = &self.and {
                    for filter in filters {
                        ::graphql_orm::graphql::orm::DatabaseFilter::validate(filter)?;
                    }
                }
                if let Some(filters) = &self.or {
                    for filter in filters {
                        ::graphql_orm::graphql::orm::DatabaseFilter::validate(filter)?;
                    }
                }
                if let Some(filter) = &self.not {
                    ::graphql_orm::graphql::orm::DatabaseFilter::validate(filter.as_ref())?;
                }
                Ok(())
            }

            fn to_sql_conditions(&self) -> (Vec<String>, Vec<::graphql_orm::graphql::orm::SqlValue>) {
                self.__gom_sql_conditions(false)
            }

            fn is_empty(&self) -> bool {
                self.__gom_is_empty()
            }

            #keyset_filter_identity_impl

            fn has_request_authorized_predicates(&self) -> bool {
                self.__gom_has_related_filters()
                    && ::graphql_orm::graphql::orm::DatabaseFilter::validate(self).is_ok()
            }

            fn referenced_fields(&self) -> Vec<&'static str> {
                let mut fields = Vec::new();
                #(#filter_referenced_field_checks)*
                if let Some(and_filters) = &self.and {
                    for filter in and_filters {
                        fields.extend(
                            ::graphql_orm::graphql::orm::DatabaseFilter::referenced_fields(filter)
                        );
                    }
                }
                if let Some(or_filters) = &self.or {
                    for filter in or_filters {
                        fields.extend(
                            ::graphql_orm::graphql::orm::DatabaseFilter::referenced_fields(filter)
                        );
                    }
                }
                if let Some(filter) = &self.not {
                    fields.extend(
                        ::graphql_orm::graphql::orm::DatabaseFilter::referenced_fields(filter.as_ref())
                    );
                }
                fields.sort_unstable();
                fields.dedup();
                fields
            }

            fn requires_in_memory_filtering(
                &self,
                backend: ::graphql_orm::graphql::orm::DatabaseBackend,
            ) -> bool {
                !backend.supports_native_spatial_predicates() && self.__gom_contains_spatial_filter()
            }

            fn to_sql_prefilter_conditions(
                &self,
                backend: ::graphql_orm::graphql::orm::DatabaseBackend,
            ) -> (Vec<String>, Vec<::graphql_orm::graphql::orm::SqlValue>) {
                if !self.requires_in_memory_filtering(backend) {
                    return self.to_sql_conditions();
                }
                self.__gom_to_sql_prefilter_conditions()
            }

            fn matches_entity(
                &self,
                entity: &(dyn ::std::any::Any + Send + Sync),
            ) -> ::graphql_orm::Result<bool> {
                let calendar_today = ::graphql_orm::chrono::DateTime::<::graphql_orm::chrono::Utc>::from(
                    ::std::time::SystemTime::now()
                ).date_naive();
                self.matches_entity_at(entity, calendar_today)
            }

            fn matches_entity_at(
                &self,
                entity: &(dyn ::std::any::Any + Send + Sync),
                calendar_today: ::graphql_orm::chrono::NaiveDate,
            ) -> ::graphql_orm::Result<bool> {
                let entity = entity.downcast_ref::<#struct_name>().ok_or_else(|| {
                    ::graphql_orm::sqlx::Error::Decode(Box::new(::std::io::Error::new(
                        ::std::io::ErrorKind::InvalidData,
                        concat!("in-memory filter entity type mismatch for ", stringify!(#struct_name)),
                    )))
                })?;
                Ok(self.__gom_matches_entity(entity, calendar_today)? == Some(true))
            }
        }

        impl #where_input_name {
            /// Resolve related-row predicates under the current request authority.
            /// Relation filters require complete SQL visibility and reject residual row policies.
            #[doc(hidden)]
            pub fn __gom_prepare_read<'a>(
                &'a mut self,
                ctx: &'a ::graphql_orm::async_graphql::Context<'_>,
                depth: usize,
                budget: &'a mut usize,
            ) -> ::std::pin::Pin<Box<dyn ::std::future::Future<Output = ::graphql_orm::async_graphql::Result<()>> + Send + 'a>> {
                Box::pin(async move {
                    if depth == 0 && !self.__gom_has_related_filters() { return Ok(()); }
                    if depth > 8 || *budget == 0 {
                        return Err(::graphql_orm::graphql::errors::OrmPublicError::new(
                            ::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput).into_graphql_error());
                    }
                    *budget -= 1;
                    #related_clear
                    #(#related_prepare)*
                    for filters in [&mut self.and, &mut self.or].into_iter().flatten() {
                        for filter in filters { filter.__gom_prepare_read(ctx, depth + 1, budget).await?; }
                    }
                    if let Some(filter) = self.not.as_mut() {
                        filter.__gom_prepare_read(ctx, depth + 1, budget).await?;
                    }
                    if self.__gom_has_related_filters() && self.__gom_contains_spatial_filter() {
                        return Err(::graphql_orm::graphql::errors::OrmPublicError::new(
                            ::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput).into_graphql_error());
                    }
                    Ok(())
                })
            }
            #[doc(hidden)]
            pub(crate) fn __gom_require_scalar_write_filter(&self) -> ::std::result::Result<(), ::graphql_orm::graphql::errors::OrmPublicError> {
                if self.__gom_has_related_filters() {
                    return Err(::graphql_orm::graphql::errors::OrmPublicError::with_message(
                            ::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput,
                            "Related-row filters are not supported for bulk writes",
                    ));
                }
                Ok(())
            }

            fn __gom_has_related_filters(&self) -> bool {
                false #(|| #related_present)*
                    || self.and.as_ref().is_some_and(|fs| fs.iter().any(Self::__gom_has_related_filters))
                    || self.or.as_ref().is_some_and(|fs| fs.iter().any(Self::__gom_has_related_filters))
                    || self.not.as_ref().is_some_and(|f| f.__gom_has_related_filters())
            }

            fn __gom_to_sql_prefilter_conditions(&self) -> (Vec<String>, Vec<::graphql_orm::graphql::orm::SqlValue>) {
                self.__gom_sql_conditions(true)
            }

            fn __gom_sql_conditions(&self, _gom_spatial_prefilter: bool) -> (Vec<String>, Vec<::graphql_orm::graphql::orm::SqlValue>) {
                if ::graphql_orm::graphql::orm::DatabaseFilter::validate(self).is_err() {
                    return (vec!["1 = 0".to_owned()], Vec::new());
                }
                let mut conditions = Vec::new();
                let mut values = Vec::new();

                #(#related_render)*
                #(#filter_to_sql)*

                // Handle And
                if let Some(ref and_filters) = self.and {
                    for filter in and_filters {
                        let (sub_conds, sub_vals) = filter.__gom_sql_conditions(_gom_spatial_prefilter);
                        conditions.extend(sub_conds);
                        values.extend(sub_vals);
                    }
                }

                if !_gom_spatial_prefilter {
                    // Handle Or
                    if let Some(ref or_filters) = self.or {
                        let mut or_parts = Vec::new();
                        for filter in or_filters {
                            let (sub_conds, sub_vals) = filter.__gom_sql_conditions(_gom_spatial_prefilter);
                            // An empty branch is true. Keep it explicit so earlier
                            // binds retain their numbered placeholder positions.
                            or_parts.push(if sub_conds.is_empty() {
                                "1 = 1".to_owned()
                            } else {
                                format!("({})", sub_conds.join(" AND "))
                            });
                            values.extend(sub_vals);
                        }
                        conditions.push(if or_parts.is_empty() {
                            "1 = 0".to_owned()
                        } else {
                            format!("({})", or_parts.join(" OR "))
                        });
                    }

                    // Handle Not
                    if let Some(ref not_filter) = self.not {
                        let (sub_conds, sub_vals) = not_filter.__gom_sql_conditions(_gom_spatial_prefilter);
                        if !sub_conds.is_empty() {
                            conditions.push(format!("NOT ({})", sub_conds.join(" AND ")));
                            values.extend(sub_vals);
                        }
                    }

                }
                (conditions, values)
            }

            fn __gom_is_empty(&self) -> bool {
                #(#filter_is_empty_checks)*

                if let Some(ref and_filters) = self.and {
                    if and_filters.iter().any(|filter| !filter.__gom_is_empty()) {
                        return false;
                    }
                }

                if let Some(ref or_filters) = self.or {
                    if or_filters.is_empty() || or_filters.iter().any(|filter| !filter.__gom_is_empty()) {
                        return false;
                    }
                }

                if let Some(ref not_filter) = self.not {
                    if !not_filter.__gom_is_empty() {
                        return false;
                    }
                }

                true
            }

            fn __gom_contains_spatial_filter(&self) -> bool {
                #(#filter_contains_spatial_checks)*

                if let Some(ref and_filters) = self.and {
                    if and_filters.iter().any(|filter| filter.__gom_contains_spatial_filter()) {
                        return true;
                    }
                }

                if let Some(ref or_filters) = self.or {
                    if or_filters.iter().any(|filter| filter.__gom_contains_spatial_filter()) {
                        return true;
                    }
                }

                if let Some(ref not_filter) = self.not {
                    if not_filter.__gom_contains_spatial_filter() {
                        return true;
                    }
                }

                false
            }

            fn __gom_matches_entity(
                &self,
                entity: &#struct_name,
                calendar_today: ::graphql_orm::chrono::NaiveDate,
            ) -> ::graphql_orm::Result<Option<bool>> {
                // Refuse the entire tree: Or/Not must not turn an unsupported
                // relation predicate into a successful in-memory match.
                if self.__gom_has_related_filters() { return Ok(None); }
                let mut _gom_unknown = false;
                #(#filter_to_entity_match)*

                if let Some(ref and_filters) = self.and {
                    for filter in and_filters {
                        match filter.__gom_matches_entity(entity, calendar_today)? {
                            Some(true) => {}
                            Some(false) => return Ok(Some(false)),
                            None => _gom_unknown = true,
                        }
                    }
                }

                if let Some(ref or_filters) = self.or {
                    let mut matched = false;
                    let mut unknown = false;
                    for filter in or_filters {
                        match filter.__gom_matches_entity(entity, calendar_today)? {
                            Some(true) => {
                                matched = true;
                                break;
                            }
                            Some(false) => {}
                            None => unknown = true,
                        }
                    }
                    if !matched {
                        if unknown {
                            _gom_unknown = true;
                        } else {
                            return Ok(Some(false));
                        }
                    }
                }

                if let Some(ref not_filter) = self.not {
                    match not_filter.__gom_matches_entity(entity, calendar_today)? {
                        Some(true) => return Ok(Some(false)),
                        Some(false) => {}
                        None => _gom_unknown = true,
                    }
                }

                Ok(if _gom_unknown { None } else { Some(true) })
            }
        }

        impl ::graphql_orm::graphql::orm::DatabaseEntity for #struct_name {
            const TABLE_NAME: &'static str = #table_name;
            const PLURAL_NAME: &'static str = #plural_name;
            const PRIMARY_KEY: &'static str = #primary_key;
            const PRIMARY_KEYS: &'static [&'static str] = &[#(#primary_key_literals),*];
            const SCHEMA_POLICY: Option<::graphql_orm::graphql::orm::SchemaPolicy> = #schema_policy_const;
            const DEFAULT_SORT: &'static str = #default_sort;

            fn column_names() -> &'static [&'static str] {
                &[#(#columns_array),*]
            }
        }

        impl #struct_name {
            pub(crate) fn __gom_placeholder(index: usize) -> String {
                #placeholder_body
            }

            pub(crate) fn __gom_rebind_sql(sql: &str, start_index: usize) -> String {
                ::graphql_orm::graphql::orm::SqlDialect::normalize_sql(
                    &<#backend_marker as ::graphql_orm::graphql::orm::OrmBackend>::DIALECT,
                    sql,
                    start_index,
                )
            }

            pub(crate) fn __gom_ci_like(column: &str, placeholder: &str) -> String {
                #ci_like_body
            }

            pub(crate) fn __gom_spatial_geojson_expr(placeholder: &str, srid: i32) -> String {
                ::graphql_orm::graphql::orm::SqlDialect::spatial_geojson_expr(
                    &<#backend_marker as ::graphql_orm::graphql::orm::OrmBackend>::DIALECT,
                    placeholder,
                    srid,
                )
            }

            pub(crate) fn __gom_spatial_predicate(
                predicate: ::graphql_orm::graphql::orm::SpatialPredicate,
                column: &str,
                geometry_expr: &str,
            ) -> String {
                ::graphql_orm::graphql::orm::SqlDialect::spatial_predicate(
                    &<#backend_marker as ::graphql_orm::graphql::orm::OrmBackend>::DIALECT,
                    predicate,
                    column,
                    geometry_expr,
                )
            }

            pub(crate) fn __gom_supports_native_spatial_predicates() -> bool {
                <#backend_marker as ::graphql_orm::graphql::orm::OrmBackend>::DIALECT
                    .supports_native_spatial_predicates()
            }

            pub(crate) fn __gom_spatial_sql_value<E>(
                value: &::graphql_orm::serde_json::Value,
                spatial: ::graphql_orm::graphql::orm::SpatialColumnDef,
            ) -> Result<::graphql_orm::graphql::orm::SqlValue, E>
            where
                E: ::graphql_orm::graphql::orm::OrmResultError,
            {
                #spatial_sql_value_body
            }

            pub(crate) fn __gom_bool_sql_value(value: bool) -> ::graphql_orm::graphql::orm::SqlValue {
                #bool_sql_value_body
            }

            pub(crate) fn __gom_current_updated_at_expr() -> &'static str {
                #current_updated_at_runtime
            }

        }

        impl ::graphql_orm::graphql::orm::DatabaseSchema for #struct_name {
            fn columns() -> &'static [::graphql_orm::graphql::orm::ColumnDef] {
                static COLUMNS: &[::graphql_orm::graphql::orm::ColumnDef] = &[
                    #(#column_defs),*
                ];
                COLUMNS
            }

            fn indexes() -> &'static [::graphql_orm::graphql::orm::IndexDef] {
                static INDEXES: &[::graphql_orm::graphql::orm::IndexDef] = &[
                    #(#index_defs),*
                ];
                INDEXES
            }

            fn composite_unique_indexes() -> &'static [&'static [&'static str]] {
                static UNIQUE_INDEXES: &[&[&str]] = &[
                    #(#composite_unique_indexes),*
                ];
                UNIQUE_INDEXES
            }

            fn check_constraints() -> &'static [::graphql_orm::graphql::orm::CheckConstraintDef] {
                static CONSTRAINTS: &[::graphql_orm::graphql::orm::CheckConstraintDef] = &[
                    #(#constraint_defs),*
                ];
                CONSTRAINTS
            }
        }

        impl ::graphql_orm::graphql::orm::EntityRelations for #struct_name {
            fn relation_metadata() -> &'static [::graphql_orm::graphql::orm::RelationMetadata] {
                // Inherent methods win over this local blanket fallback.
                    // No witness means no generated getter ownership.
                    #[allow(dead_code)]
                    trait OutputFallback {
                        fn __gom_exposed_relation_fields() -> &'static [&'static str] { &[] }
                    }
                    impl<T> OutputFallback for T {}
                    static RELATIONS: ::std::sync::OnceLock<::std::boxed::Box<[::graphql_orm::graphql::orm::RelationMetadata]>> =
                        ::std::sync::OnceLock::new();
                    RELATIONS.get_or_init(|| {
                        let _generated_output = #struct_name::__gom_exposed_relation_fields();
                        vec![#(#relation_metadata_defs),*].into_boxed_slice()
                    })
            }
        }

        impl ::graphql_orm::graphql::orm::Entity for #struct_name {
            fn entity_name() -> &'static str {
                #struct_name_str
            }

            fn managed_version_field() -> Option<&'static str> {
                #managed_version_field_tokens
            }

            fn metadata() -> &'static ::graphql_orm::graphql::orm::EntityMetadata {
                static METADATA: ::std::sync::OnceLock<::graphql_orm::graphql::orm::EntityMetadata> =
                    ::std::sync::OnceLock::new();
                METADATA.get_or_init(|| {
                    ::graphql_orm::graphql::orm::EntityMetadata::from_schema_with_retention::<Self>(
                        #struct_name_str,
                        #backup_enabled,
                        #backup_export_order,
                        #backup_restore_order,
                        #read_policy,
                        #write_policy,
                        #append_only,
                        #retention_policy,
                    )
                })
            }

            fn graphql_semantic_metadata() -> Option<&'static ::graphql_orm::graphql::orm::GraphqlEntitySemanticMetadata> {
                static METADATA: ::std::sync::OnceLock<
                    ::graphql_orm::graphql::orm::GraphqlEntitySemanticMetadata
                > = ::std::sync::OnceLock::new();
                Some(METADATA.get_or_init(|| {
                    ::graphql_orm::graphql::orm::GraphqlEntitySemanticMetadata {
                        generated_input_members: vec![#(#hidden_input_member_defs),*],
                        entity_name: ::std::string::ToString::to_string(#struct_name_str),
                        description: ::std::string::ToString::to_string(#entity_description),
                        default_classification: #entity_classification_tokens,
                        fields: vec![#(#semantic_field_defs),*].into_boxed_slice(),
                        label_fields: ::std::vec::Vec::new(),
                        record_version: #semantic_record_version,
                    }
                }))
            }


            fn subscription_field_policy(field: &str) -> Option<(&'static str, bool, Option<&'static str>)> {
                match field { #(#subscription_field_policy_arms)* _ => None }
            }

            fn repository_field_policies() -> &'static [::graphql_orm::graphql::orm::RepositoryFieldPolicyDef] {
                static FIELDS: &[::graphql_orm::graphql::orm::RepositoryFieldPolicyDef] = &[
                    #(#repository_field_policy_defs),*
                ];
                FIELDS
            }

            fn repository_policy_record(
                &self,
            ) -> Option<&(dyn ::std::any::Any + Send + Sync)> {
                Some(self)
            }
        }

        impl ::graphql_orm::graphql::orm::GraphqlSemanticResultTypeMetadata for #struct_name {
            fn graphql_semantic_result_type(
            ) -> &'static ::graphql_orm::graphql::orm::GraphqlEntitySemanticMetadata {
                <Self as ::graphql_orm::graphql::orm::Entity>
                    ::graphql_semantic_metadata()
                    .expect("GraphQLEntity must emit semantic metadata")
            }
        }

        impl ::graphql_orm::graphql::orm::FromSqlRow<#backend_marker> for #struct_name {
            fn from_row(row: &#row_type) -> ::graphql_orm::Result<Self> {
                #helper_import

                Ok(Self {
                    #(#from_row_fields)*
                })
            }
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn generate_projection_definitions(
    backend: BackendKind,
    entity: &syn::Ident,
    entity_meta: &EntityMetadata,
    fields: &[ParsedField],
    where_input: &syn::Ident,
    order_input: &syn::Ident,
    backend_marker: &proc_macro2::TokenStream,
    row_type: &proc_macro2::TokenStream,
    helper_import: &proc_macro2::TokenStream,
    visibility: &syn::Visibility,
) -> syn::Result<proc_macro2::TokenStream> {
    if entity_meta.projections.is_empty() {
        return Ok(quote! {});
    }

    let mut projection_names = std::collections::HashSet::new();
    let primary_keys = fields
        .iter()
        .filter(|field| field.meta.is_primary_key)
        .collect::<Vec<_>>();
    let primary_keys = if primary_keys.is_empty() {
        fields
            .iter()
            .filter(|field| {
                field
                    .field
                    .ident
                    .as_ref()
                    .is_some_and(|ident| ident == "id")
            })
            .collect::<Vec<_>>()
    } else {
        primary_keys
    };

    let mut definitions = Vec::new();
    for projection in &entity_meta.projections {
        let name = projection
            .name
            .as_deref()
            .expect("validated projection name");
        let projection_name: syn::Ident = syn::parse_str(name).map_err(|_| {
            syn::Error::new(
                entity.span(),
                format!("projection name `{name}` must be a valid Rust type identifier"),
            )
        })?;
        if !projection_names.insert(name.to_string()) {
            return Err(syn::Error::new(
                entity.span(),
                format!("duplicate projection name `{name}`"),
            ));
        }

        let mut requested = std::collections::HashSet::new();
        let mut selected = Vec::new();
        for requested_name in &projection.fields {
            if !requested.insert(requested_name.clone()) {
                return Err(syn::Error::new(
                    entity.span(),
                    format!("projection `{name}` contains duplicate field `{requested_name}`"),
                ));
            }
            let field = fields
                .iter()
                .find(|field| {
                    field
                        .field
                        .ident
                        .as_ref()
                        .is_some_and(|ident| ident == requested_name)
                })
                .ok_or_else(|| {
                    syn::Error::new(
                        entity.span(),
                        format!(
                            "projection `{name}` references unknown field `{requested_name}` on `{entity}`"
                        ),
                    )
                })?;
            if field.meta.is_relation || field.meta.skip_db {
                return Err(syn::Error::new(
                    field.field.span(),
                    format!(
                        "projection `{name}` field `{requested_name}` must be a persisted scalar field"
                    ),
                ));
            }
            selected.push(field);
        }

        let dto_fields = selected.iter().map(|selected| {
            let ident = selected.field.ident.as_ref().expect("named field");
            let ty = &selected.field.ty;
            quote! { pub #ident: #ty, }
        });
        let row_fields = selected
            .iter()
            .map(|selected| {
                let ident = selected.field.ident.as_ref().expect("named field");
                let db_column = selected
                    .meta
                    .db_column
                    .clone()
                    .unwrap_or_else(|| ident.to_string());
                generate_row_field_assignment(
                    backend,
                    ident,
                    &selected.field.ty,
                    &db_column,
                    &selected.meta,
                )
            })
            .collect::<syn::Result<Vec<_>>>()?;
        let selected_columns = selected
            .iter()
            .map(|selected| {
                let ident = selected.field.ident.as_ref().expect("named field");
                let db_column = selected
                    .meta
                    .db_column
                    .clone()
                    .unwrap_or_else(|| ident.to_string());
                let quoted = backend_render_identifier_path(backend, &db_column);
                if selected.meta.mssql_money {
                    format!("CAST({quoted} AS DECIMAL(19,4)) AS {quoted}")
                } else {
                    quoted
                }
            })
            .collect::<Vec<_>>();
        let selected_field_names = selected
            .iter()
            .map(|selected| {
                selected
                    .field
                    .ident
                    .as_ref()
                    .expect("named field")
                    .to_string()
            })
            .collect::<Vec<_>>();
        let debug_fields = selected.iter().map(|selected| {
            let ident = selected.field.ident.as_ref().expect("named field");
            let label = ident.to_string();
            let redact =
                selected.meta.sensitive || selected.meta.backup_policy.as_deref() == Some("redact");
            if redact {
                quote! { debug.field(#label, &"[redacted]"); }
            } else {
                quote! { debug.field(#label, &self.#ident); }
            }
        });

        let key_methods = if primary_keys.is_empty() {
            quote! {}
        } else {
            let key_arguments = primary_keys.iter().map(|key| {
                let ident = key.field.ident.as_ref().expect("named field");
                let ty = option_inner_type(&key.field.ty).unwrap_or(&key.field.ty);
                quote! { #ident: &#ty }
            });
            let key_values = primary_keys
                .iter()
                .map(|key| projection_lookup_sql_value(key, key.field.ident.as_ref().unwrap()));
            let transaction_key_arguments = primary_keys.iter().map(|key| {
                let ident = key.field.ident.as_ref().expect("named field");
                let ty = option_inner_type(&key.field.ty).unwrap_or(&key.field.ty);
                quote! { #ident: &#ty }
            });
            let transaction_key_values = primary_keys
                .iter()
                .map(|key| projection_lookup_sql_value(key, key.field.ident.as_ref().unwrap()));
            let single_id_alias = if primary_keys.len() == 1 {
                let key = primary_keys[0];
                let ty = option_inner_type(&key.field.ty).unwrap_or(&key.field.ty);
                quote! {
                    pub async fn find_by_id(
                        db: &::graphql_orm::db::Database<#backend_marker>,
                        id: &#ty,
                    ) -> ::graphql_orm::Result<Option<Self>> {
                        Self::find_by_key(db, id).await
                    }

                    pub async fn find_by_id_in(
                        transaction: &mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                        id: &#ty,
                    ) -> ::graphql_orm::Result<Option<Self>> {
                        Self::find_by_key_in(transaction, id).await
                    }
                }
            } else {
                quote! {}
            };
            quote! {
                pub async fn find_by_key(
                    db: &::graphql_orm::db::Database<#backend_marker>,
                    #(#key_arguments),*
                ) -> ::graphql_orm::Result<Option<Self>> {
                    let values = vec![#(#key_values),*];
                    Self::query(db).__where_primary_key(values).fetch_optional_one().await
                }

                pub async fn find_by_key_in(
                    transaction: &mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    #(#transaction_key_arguments),*
                ) -> ::graphql_orm::Result<Option<Self>> {
                    let values = vec![#(#transaction_key_values),*];
                    transaction.project::<Self>()
                        .__where_primary_key(values)
                        .fetch_optional_one()
                        .await
                }

                #single_id_alias
            }
        };

        let unique_methods = fields
            .iter()
            .filter(|field| {
                field.meta.unique
                    && !field.meta.is_primary_key
                    && !field.meta.is_relation
                    && !field.meta.skip_db
            })
            .enumerate()
            .map(|(unique_index, field)| {
                let ident = field.field.ident.as_ref().expect("named field");
                let method = syn::Ident::new(&format!("find_by_{ident}"), ident.span());
                let transaction_method =
                    syn::Ident::new(&format!("find_by_{ident}_in"), ident.span());
                let ty = option_inner_type(&field.field.ty).unwrap_or(&field.field.ty);
                let value = projection_lookup_sql_value(field, ident);
                let transaction_value = projection_lookup_sql_value(field, ident);
                quote! {
                    pub async fn #method(
                        db: &::graphql_orm::db::Database<#backend_marker>,
                        #ident: &#ty,
                    ) -> ::graphql_orm::Result<Option<Self>> {
                        Self::query(db)
                            .__where_unique(#unique_index, #value)
                            .fetch_optional_one()
                            .await
                    }

                    pub async fn #transaction_method(
                        transaction: &mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                        #ident: &#ty,
                    ) -> ::graphql_orm::Result<Option<Self>> {
                        transaction.project::<Self>()
                            .__where_unique(#unique_index, #transaction_value)
                            .fetch_optional_one()
                            .await
                    }
                }
            })
            .collect::<Vec<_>>();

        let unique_columns = fields
            .iter()
            .filter(|field| {
                field.meta.unique
                    && !field.meta.is_primary_key
                    && !field.meta.is_relation
                    && !field.meta.skip_db
            })
            .map(|field| {
                let ident = field.field.ident.as_ref().expect("named field");
                field
                    .meta
                    .db_column
                    .clone()
                    .unwrap_or_else(|| ident.to_string())
            });

        definitions.push(quote! {
            #[derive(Clone, PartialEq)]
            #visibility struct #projection_name {
                #(#dto_fields)*
            }

            impl ::std::fmt::Debug for #projection_name {
                fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                    let mut debug = formatter.debug_struct(#name);
                    #(#debug_fields)*
                    debug.finish()
                }
            }

            impl ::graphql_orm::graphql::orm::FromSqlRow<#backend_marker> for #projection_name {
                fn from_row(row: &#row_type) -> ::graphql_orm::Result<Self> {
                    #helper_import
                    Ok(Self {
                        #(#row_fields)*
                    })
                }
            }

            impl ::graphql_orm::graphql::orm::ReadProjection<#backend_marker> for #projection_name {
                type Entity = #entity;
                type Filter = #where_input;
                type Order = #order_input;
                fn entity_type_id() -> Option<::std::any::TypeId> {
                    Some(::std::any::TypeId::of::<#entity>())
                }
                const COLUMNS: &'static [&'static str] = &[#(#selected_columns),*];
                const FIELD_NAMES: &'static [&'static str] = &[#(#selected_field_names),*];
                const UNIQUE_COLUMNS: &'static [&'static str] = &[#(#unique_columns),*];
            }

            #[allow(clippy::ptr_arg)]
            impl #projection_name {
                pub fn query(
                    db: &::graphql_orm::db::Database<#backend_marker>,
                ) -> ::graphql_orm::graphql::orm::ProjectionQuery<'_, Self, #backend_marker> {
                    ::graphql_orm::graphql::orm::ProjectionQuery::new(db)
                }

                pub fn query_with_auth<'a>(
                    db: &'a ::graphql_orm::db::Database<#backend_marker>,
                    auth: Option<&::graphql_orm::graphql::orm::DbAuthContext>,
                ) -> ::graphql_orm::graphql::orm::ProjectionQuery<'a, Self, #backend_marker> {
                    ::graphql_orm::graphql::orm::ProjectionQuery::new(db).with_auth(auth)
                }

                #key_methods
                #(#unique_methods)*
            }
        });
    }

    Ok(quote! { #(#definitions)* })
}

fn projection_lookup_sql_value(
    field: &ParsedField,
    argument: &syn::Ident,
) -> proc_macro2::TokenStream {
    let ty = option_inner_type(&field.field.ty).unwrap_or(&field.field.ty);
    if is_byte_vec_type(ty) {
        quote! { ::graphql_orm::graphql::orm::SqlValue::Bytes(#argument.clone()) }
    } else if is_uuid_type(ty) {
        quote! { ::graphql_orm::graphql::orm::SqlValue::Uuid(*#argument) }
    } else if is_bool_type(ty) {
        quote! { ::graphql_orm::graphql::orm::SqlValue::Bool(*#argument) }
    } else {
        let type_name = quote! { #ty }.to_string().replace(' ', "");
        match type_name.as_str() {
            "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64" | "usize" => {
                quote! { ::graphql_orm::graphql::orm::SqlValue::Int(*#argument as i64) }
            }
            "f32" | "f64" => {
                quote! { ::graphql_orm::graphql::orm::SqlValue::Float(*#argument as f64) }
            }
            "String" => {
                quote! { ::graphql_orm::graphql::orm::SqlValue::String(#argument.clone()) }
            }
            _ => quote! {
                ::graphql_orm::graphql::orm::SqlValue::String(#argument.to_string())
            },
        }
    }
}

// ============================================================================
// Filter Field Generation
// ============================================================================

#[allow(clippy::too_many_arguments)]
fn generate_filter_field(
    backend: BackendKind,
    struct_name: &syn::Ident,
    field_name: &syn::Ident,
    field_type: &syn::Type,
    graphql_name: &str,
    db_col: &str,
    filter_type: &str,
    field_meta: &FieldMetadata,
) -> syn::Result<(
    proc_macro2::TokenStream,
    proc_macro2::TokenStream,
    proc_macro2::TokenStream,
    proc_macro2::TokenStream,
    proc_macro2::TokenStream,
)> {
    let filter_field_name = rust_ident_from_graphql_name(graphql_name, field_name.span());
    let backend_expr = match backend {
        BackendKind::Sqlite => quote! { ::graphql_orm::graphql::orm::DatabaseBackend::Sqlite },
        BackendKind::Postgres => quote! { ::graphql_orm::graphql::orm::DatabaseBackend::Postgres },
        BackendKind::Mssql => quote! { ::graphql_orm::graphql::orm::DatabaseBackend::Mssql },
    };
    let is_empty_check = quote! {
        if self.#filter_field_name.is_some() {
            return false;
        }
    };
    let no_spatial_check = quote! {};

    match filter_type {
        "decimal" => {
            let Some(_) = field_meta.decimal else {
                return Err(syn::Error::new(
                    field_name.span(),
                    "filterable(type = \"decimal\") requires decimal metadata",
                ));
            };
            let definition = decimal_definition_tokens(field_meta);
            let input = quote! {
                #[graphql(name = #graphql_name)]
                pub #filter_field_name: Option<::graphql_orm::graphql::filters::DecimalFilter>,
            };
            let sql = quote! {
                if let Some(ref f) = self.#filter_field_name {
                    let definition = #definition;
                    for (value, operator) in [
                        (f.eq, "="),
                        (f.ne, "!="),
                        (f.lt, "<"),
                        (f.lte, "<="),
                        (f.gt, ">"),
                        (f.gte, ">="),
                    ] {
                        if let Some(value) = value {
                            match ::graphql_orm::graphql::orm::DecimalValue::new(value, definition) {
                                Ok(value) => {
                                    let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                                    conditions.push(format!("{} {} {}", #db_col, operator, placeholder));
                                    values.push(::graphql_orm::graphql::orm::SqlValue::Decimal(value));
                                }
                                Err(_) => conditions.push("1 = 0".to_owned()),
                            }
                        }
                    }
                    if let Some(is_null) = f.is_null {
                        conditions.push(format!(
                            "{} IS {}NULL",
                            #db_col,
                            if is_null { "" } else { "NOT " }
                        ));
                    }
                }
            };
            Ok((input, sql, quote! {}, is_empty_check, no_spatial_check))
        }
        "spatial" => {
            let Some(spatial) = &field_meta.spatial else {
                return Err(syn::Error::new(
                    field_name.span(),
                    "filterable(type = \"spatial\") requires #[graphql_orm(spatial(...))]",
                ));
            };
            let srid = spatial.srid;
            let input = quote! {
                #[graphql(name = #graphql_name)]
                pub #filter_field_name: Option<::graphql_orm::graphql::filters::SpatialFilter>,
            };
            let sql = quote! {
                if let Some(ref f) = self.#filter_field_name {
                    if #struct_name::__gom_supports_native_spatial_predicates() {
                        if let Some(ref v) = f.equals {
                            let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                            let geometry = #struct_name::__gom_spatial_geojson_expr(&placeholder, #srid);
                            conditions.push(#struct_name::__gom_spatial_predicate(
                                ::graphql_orm::graphql::orm::SpatialPredicate::Equals,
                                #db_col,
                                &geometry,
                            ));
                            values.push(::graphql_orm::graphql::orm::SqlValue::Json(v.0.clone()));
                        }
                        if let Some(ref v) = f.disjoint {
                            let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                            let geometry = #struct_name::__gom_spatial_geojson_expr(&placeholder, #srid);
                            conditions.push(#struct_name::__gom_spatial_predicate(
                                ::graphql_orm::graphql::orm::SpatialPredicate::Disjoint,
                                #db_col,
                                &geometry,
                            ));
                            values.push(::graphql_orm::graphql::orm::SqlValue::Json(v.0.clone()));
                        }
                        if let Some(ref v) = f.intersects {
                            let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                            let geometry = #struct_name::__gom_spatial_geojson_expr(&placeholder, #srid);
                            conditions.push(#struct_name::__gom_spatial_predicate(
                                ::graphql_orm::graphql::orm::SpatialPredicate::Intersects,
                                #db_col,
                                &geometry,
                            ));
                            values.push(::graphql_orm::graphql::orm::SqlValue::Json(v.0.clone()));
                        }
                        if let Some(ref v) = f.touches {
                            let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                            let geometry = #struct_name::__gom_spatial_geojson_expr(&placeholder, #srid);
                            conditions.push(#struct_name::__gom_spatial_predicate(
                                ::graphql_orm::graphql::orm::SpatialPredicate::Touches,
                                #db_col,
                                &geometry,
                            ));
                            values.push(::graphql_orm::graphql::orm::SqlValue::Json(v.0.clone()));
                        }
                        if let Some(ref v) = f.crosses {
                            let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                            let geometry = #struct_name::__gom_spatial_geojson_expr(&placeholder, #srid);
                            conditions.push(#struct_name::__gom_spatial_predicate(
                                ::graphql_orm::graphql::orm::SpatialPredicate::Crosses,
                                #db_col,
                                &geometry,
                            ));
                            values.push(::graphql_orm::graphql::orm::SqlValue::Json(v.0.clone()));
                        }
                        if let Some(ref v) = f.within {
                            let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                            let geometry = #struct_name::__gom_spatial_geojson_expr(&placeholder, #srid);
                            conditions.push(#struct_name::__gom_spatial_predicate(
                                ::graphql_orm::graphql::orm::SpatialPredicate::Within,
                                #db_col,
                                &geometry,
                            ));
                            values.push(::graphql_orm::graphql::orm::SqlValue::Json(v.0.clone()));
                        }
                        if let Some(ref v) = f.contains {
                            let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                            let geometry = #struct_name::__gom_spatial_geojson_expr(&placeholder, #srid);
                            conditions.push(#struct_name::__gom_spatial_predicate(
                                ::graphql_orm::graphql::orm::SpatialPredicate::Contains,
                                #db_col,
                                &geometry,
                            ));
                            values.push(::graphql_orm::graphql::orm::SqlValue::Json(v.0.clone()));
                        }
                        if let Some(ref v) = f.overlaps {
                            let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                            let geometry = #struct_name::__gom_spatial_geojson_expr(&placeholder, #srid);
                            conditions.push(#struct_name::__gom_spatial_predicate(
                                ::graphql_orm::graphql::orm::SpatialPredicate::Overlaps,
                                #db_col,
                                &geometry,
                            ));
                            values.push(::graphql_orm::graphql::orm::SqlValue::Json(v.0.clone()));
                        }
                    }
                    if let Some(is_null) = f.is_null {
                        if is_null {
                            conditions.push(format!("{} IS NULL", #db_col));
                        } else {
                            conditions.push(format!("{} IS NOT NULL", #db_col));
                        }
                    }
                }
            };
            let value_expr = if is_option_type(field_type) {
                quote! { entity.#field_name.as_ref() }
            } else {
                quote! { Some(&entity.#field_name) }
            };
            let geometry_type =
                spatial_geometry_type_tokens(&spatial.geometry_type, field_name.span())?;
            let entity_match = if backend == BackendKind::Sqlite {
                quote! {
                    if let Some(ref f) = self.#filter_field_name {
                        if !::graphql_orm::graphql::orm::spatial::spatial_filter_matches_value(
                            #value_expr,
                            f,
                            ::graphql_orm::graphql::orm::SpatialColumnDef::geometry(#geometry_type, #srid),
                        )? {
                            return Ok(Some(false));
                        }
                    }
                }
            } else {
                quote! {}
            };
            let contains_spatial_check = quote! {
                if self.#filter_field_name.is_some() {
                    return true;
                }
            };
            Ok((
                input,
                sql,
                entity_match,
                is_empty_check,
                contains_spatial_check,
            ))
        }
        "bytes" => {
            let input = quote! {
                #[graphql(name = #graphql_name)]
                pub #filter_field_name: Option<::graphql_orm::graphql::filters::BytesFilter>,
            };
            let sql = quote! {
                if let Some(ref f) = self.#filter_field_name {
                    if let Some(ref value) = f.eq {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(format!("{} = {}", #db_col, placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::Bytes(value.clone()));
                    }
                    if let Some(ref value) = f.ne {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(format!("{} != {}", #db_col, placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::Bytes(value.clone()));
                    }
                    for (list, negated) in [(&f.in_list, false), (&f.not_in, true)] {
                        if let Some(list) = list {
                            if list.is_empty() && !negated {
                                conditions.push("1 = 0".to_owned());
                            } else if !list.is_empty() {
                                let base = values.len() + 1;
                                let placeholders = (0..list.len())
                                    .map(|offset| #struct_name::__gom_placeholder(base + offset))
                                    .collect::<Vec<_>>();
                                conditions.push(format!(
                                    "{} {}IN ({})",
                                    #db_col,
                                    if negated { "NOT " } else { "" },
                                    placeholders.join(", ")
                                ));
                                values.extend(list.iter().cloned().map(
                                    ::graphql_orm::graphql::orm::SqlValue::Bytes
                                ));
                            }
                        }
                    }
                    if let Some(is_null) = f.is_null {
                        conditions.push(format!(
                            "{} IS {}NULL",
                            #db_col,
                            if is_null { "" } else { "NOT " }
                        ));
                    }
                }
            };
            let entity_match = if is_option_type(field_type) {
                quote! {
                    if let Some(ref filter) = self.#filter_field_name {
                        let value = entity.#field_name.as_ref();
                        if filter.eq.as_ref().is_some_and(|expected| value != Some(expected))
                            || filter.ne.as_ref().is_some_and(|expected| value == Some(expected))
                            || filter.in_list.as_ref().is_some_and(|list| value.is_none_or(|value| !list.contains(value)))
                            || filter.not_in.as_ref().is_some_and(|list| value.is_some_and(|value| list.contains(value)))
                            || filter.is_null.is_some_and(|expected| expected != value.is_none())
                        {
                            return Ok(Some(false));
                        }
                    }
                }
            } else {
                quote! {
                    if let Some(ref filter) = self.#filter_field_name {
                        let value = &entity.#field_name;
                        if filter.eq.as_ref().is_some_and(|expected| value != expected)
                            || filter.ne.as_ref().is_some_and(|expected| value == expected)
                            || filter.in_list.as_ref().is_some_and(|list| !list.contains(value))
                            || filter.not_in.as_ref().is_some_and(|list| list.contains(value))
                            || filter.is_null == Some(true)
                        {
                            return Ok(Some(false));
                        }
                    }
                }
            };
            Ok((input, sql, entity_match, is_empty_check, no_spatial_check))
        }
        "string" => {
            let input = quote! {
                #[graphql(name = #graphql_name)]
                pub #filter_field_name: Option<::graphql_orm::graphql::filters::StringFilter>,
            };
            let sql = quote! {
                if let Some(ref f) = self.#filter_field_name {
                    ::graphql_orm::graphql::orm::__gom_render_string_filter(
                        f, #backend_expr, #db_col, &mut conditions, &mut values,
                    );
                }
            };
            let value_expr = if is_option_type(field_type) {
                quote! { entity.#field_name.as_deref() }
            } else {
                quote! { Some(entity.#field_name.as_str()) }
            };
            let entity_match = if backend == BackendKind::Sqlite {
                quote! {
                    if let Some(ref f) = self.#filter_field_name {
                        if !::graphql_orm::graphql::orm::spatial::string_filter_matches(#value_expr, f) {
                            return Ok(Some(false));
                        }
                    }
                }
            } else {
                // Native backends do not use the SQLite residual matcher, but
                // explicit empty inclusion is always a definite non-match.
                quote! {
                    if self.#filter_field_name.as_ref().is_some_and(|filter|
                        filter.in_list.as_ref().is_some_and(Vec::is_empty)) {
                        return Ok(Some(false));
                    }
                }
            };
            Ok((input, sql, entity_match, is_empty_check, no_spatial_check))
        }
        "number" => {
            let input = quote! {
                #[graphql(name = #graphql_name)]
                pub #filter_field_name: Option<::graphql_orm::graphql::filters::IntFilter>,
            };
            let sql = quote! {
                if let Some(ref f) = self.#filter_field_name {
                    ::graphql_orm::graphql::orm::__gom_render_number_filter(
                        f, #backend_expr, #db_col, &mut conditions, &mut values,
                    );
                }
            };
            let value_expr = if is_option_type(field_type) {
                quote! { entity.#field_name.map(|value| value as i64) }
            } else {
                quote! { Some(entity.#field_name as i64) }
            };
            let entity_match = if backend == BackendKind::Sqlite {
                quote! {
                    if let Some(ref f) = self.#filter_field_name {
                        if !::graphql_orm::graphql::orm::spatial::int_filter_matches(#value_expr, f) {
                            return Ok(Some(false));
                        }
                    }
                }
            } else {
                // Native backends do not use the SQLite residual matcher, but
                // explicit empty inclusion is always a definite non-match.
                quote! {
                    if self.#filter_field_name.as_ref().is_some_and(|filter|
                        filter.in_list.as_ref().is_some_and(Vec::is_empty)) {
                        return Ok(Some(false));
                    }
                }
            };
            Ok((input, sql, entity_match, is_empty_check, no_spatial_check))
        }
        "int64" => {
            let inner = option_inner_type(field_type).unwrap_or(field_type);
            if !type_path_last_ident(inner).is_some_and(|name| name == "i64") {
                return Err(syn::Error::new_spanned(
                    field_type,
                    "int64 filter requires i64 or Option<i64>",
                ));
            }
            let input = quote! {
                #[graphql(name = #graphql_name)]
                pub #filter_field_name: Option<::graphql_orm::graphql::filters::Int64Filter>,
            };
            let sql = quote! {
                if let Some(ref f) = self.#filter_field_name {
                    if let Some(v) = f.eq {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(format!("{} = {}", #db_col, placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::Int(v));
                    }
                    if let Some(v) = f.ne {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(format!("{} != {}", #db_col, placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::Int(v));
                    }
                    if let Some(v) = f.lt {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(format!("{} < {}", #db_col, placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::Int(v));
                    }
                    if let Some(v) = f.lte {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(format!("{} <= {}", #db_col, placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::Int(v));
                    }
                    if let Some(v) = f.gt {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(format!("{} > {}", #db_col, placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::Int(v));
                    }
                    if let Some(v) = f.gte {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(format!("{} >= {}", #db_col, placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::Int(v));
                    }
                    if let Some(ref list) = f.in_list {
                        if list.is_empty() {
                            conditions.push("1 = 0".to_owned());
                        } else {
                            let base_index = values.len() + 1;
                            let placeholders: Vec<String> = (0..list.len())
                                .map(|offset| #struct_name::__gom_placeholder(base_index + offset))
                                .collect();
                            conditions.push(format!("{} IN ({})", #db_col, placeholders.join(", ")));
                            for v in list {
                                values.push(::graphql_orm::graphql::orm::SqlValue::Int(*v));
                            }
                        }
                    }
                    if let Some(ref list) = f.not_in {
                        if !list.is_empty() {
                            let base_index = values.len() + 1;
                            let placeholders: Vec<String> = (0..list.len())
                                .map(|offset| #struct_name::__gom_placeholder(base_index + offset))
                                .collect();
                            conditions.push(format!("{} NOT IN ({})", #db_col, placeholders.join(", ")));
                            for v in list {
                                values.push(::graphql_orm::graphql::orm::SqlValue::Int(*v));
                            }
                        }
                    }
                    // IsNull / IsNotNull
                    if let Some(is_null) = f.is_null {
                        if is_null {
                            conditions.push(format!("{} IS NULL", #db_col));
                        } else {
                            conditions.push(format!("{} IS NOT NULL", #db_col));
                        }
                    }
                }
            };
            let value_expr = if is_option_type(field_type) {
                quote! { entity.#field_name }
            } else {
                quote! { Some(entity.#field_name) }
            };
            let entity_match = if backend == BackendKind::Sqlite {
                quote! {
                    if let Some(ref f) = self.#filter_field_name {
                        f.validate()?;
                        match ::graphql_orm::graphql::orm::spatial::int64_filter_matches(#value_expr, f) {
                            Some(true) => {},
                            Some(false) => return Ok(Some(false)),
                            None => _gom_unknown = true,
                        }
                    }
                }
            } else {
                // Native backends do not use the SQLite residual matcher, but
                // explicit empty inclusion is always a definite non-match.
                quote! {
                    if self.#filter_field_name.as_ref().is_some_and(|filter|
                        filter.in_list.as_ref().is_some_and(Vec::is_empty)) {
                        return Ok(Some(false));
                    }
                }
            };
            Ok((input, sql, entity_match, is_empty_check, no_spatial_check))
        }
        "uuid" => {
            let input = quote! {
                #[graphql(name = #graphql_name)]
                pub #filter_field_name: Option<::graphql_orm::graphql::filters::UuidFilter>,
            };
            let sql = quote! {
                if let Some(ref f) = self.#filter_field_name {
                    if let Some(v) = f.eq {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(format!("{} = {}", #db_col, placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::Uuid(v));
                    }
                    if let Some(v) = f.ne {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(format!("{} != {}", #db_col, placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::Uuid(v));
                    }
                    if let Some(ref list) = f.in_list {
                        if list.is_empty() {
                            conditions.push("1 = 0".to_owned());
                        } else {
                            let base_index = values.len() + 1;
                            let placeholders: Vec<String> = (0..list.len())
                                .map(|offset| #struct_name::__gom_placeholder(base_index + offset))
                                .collect();
                            conditions.push(format!("{} IN ({})", #db_col, placeholders.join(", ")));
                            for v in list {
                                values.push(::graphql_orm::graphql::orm::SqlValue::Uuid(*v));
                            }
                        }
                    }
                    if let Some(ref list) = f.not_in {
                        if !list.is_empty() {
                            let base_index = values.len() + 1;
                            let placeholders: Vec<String> = (0..list.len())
                                .map(|offset| #struct_name::__gom_placeholder(base_index + offset))
                                .collect();
                            conditions.push(format!("{} NOT IN ({})", #db_col, placeholders.join(", ")));
                            for v in list {
                                values.push(::graphql_orm::graphql::orm::SqlValue::Uuid(*v));
                            }
                        }
                    }
                    if let Some(is_null) = f.is_null {
                        if is_null {
                            conditions.push(format!("{} IS NULL", #db_col));
                        } else {
                            conditions.push(format!("{} IS NOT NULL", #db_col));
                        }
                    }
                }
            };
            let value_expr = if is_option_type(field_type) {
                quote! { entity.#field_name }
            } else {
                quote! { Some(entity.#field_name) }
            };
            let entity_match = if backend == BackendKind::Sqlite {
                quote! {
                    if let Some(ref f) = self.#filter_field_name {
                        if !::graphql_orm::graphql::orm::spatial::uuid_filter_matches(#value_expr, f) {
                            return Ok(Some(false));
                        }
                    }
                }
            } else {
                // Native backends do not use the SQLite residual matcher, but
                // explicit empty inclusion is always a definite non-match.
                quote! {
                    if self.#filter_field_name.as_ref().is_some_and(|filter|
                        filter.in_list.as_ref().is_some_and(Vec::is_empty)) {
                        return Ok(Some(false));
                    }
                }
            };
            Ok((input, sql, entity_match, is_empty_check, no_spatial_check))
        }
        "boolean" | "bool" => {
            let input = quote! {
                #[graphql(name = #graphql_name)]
                pub #filter_field_name: Option<::graphql_orm::graphql::filters::BoolFilter>,
            };
            let sql = quote! {
                if let Some(ref f) = self.#filter_field_name {
                    ::graphql_orm::graphql::orm::__gom_render_boolean_filter(
                        f, #backend_expr, #db_col, &mut conditions, &mut values,
                    );
                }
            };
            let value_expr = if is_option_type(field_type) {
                quote! { entity.#field_name }
            } else {
                quote! { Some(entity.#field_name) }
            };
            let entity_match = if backend == BackendKind::Sqlite {
                quote! {
                    if let Some(ref f) = self.#filter_field_name {
                        if !::graphql_orm::graphql::orm::spatial::bool_filter_matches(#value_expr, f) {
                            return Ok(Some(false));
                        }
                    }
                }
            } else {
                quote! {}
            };
            Ok((input, sql, entity_match, is_empty_check, no_spatial_check))
        }
        "date" => {
            let input = quote! {
                #[graphql(name = #graphql_name)]
                pub #filter_field_name: Option<::graphql_orm::graphql::filters::DateFilter>,
            };
            let sql = quote! {
                if let Some(ref f) = self.#filter_field_name {
                    let rendered = if _gom_spatial_prefilter {
                        f.render_sql_prefilter(#backend_expr, #db_col, values.len() + 1)
                    } else {
                        f.render_sql(#backend_expr, #db_col, values.len() + 1)
                    };
                    match rendered {
                        Ok((date_conditions, date_values)) => {
                            conditions.extend(date_conditions);
                            values.extend(date_values);
                        }
                        Err(_) => {
                            // Direct rendering remains fail-closed. Query execution
                            // reports the validation error through DatabaseFilter::validate.
                            conditions.push("1 = 0".to_owned());
                        }
                    }
                }
            };
            let entity_match = if backend == BackendKind::Sqlite {
                if is_option_type(field_type) {
                    quote! {
                        if let Some(ref f) = self.#filter_field_name {
                            let __gom_date_value = entity.#field_name.as_ref().map(|value| value.to_string());
                            match ::graphql_orm::graphql::orm::spatial::date_filter_truth_at(
                                __gom_date_value.as_deref(),
                                f,
                                calendar_today,
                            ) {
                                Some(true) => {}
                                Some(false) => return Ok(Some(false)),
                                None => _gom_unknown = true,
                            }
                        }
                    }
                } else {
                    quote! {
                        if let Some(ref f) = self.#filter_field_name {
                            let __gom_date_value = entity.#field_name.to_string();
                            match ::graphql_orm::graphql::orm::spatial::date_filter_truth_at(
                                Some(__gom_date_value.as_str()),
                                f,
                                calendar_today,
                            ) {
                                Some(true) => {}
                                Some(false) => return Ok(Some(false)),
                                None => _gom_unknown = true,
                            }
                        }
                    }
                }
            } else {
                quote! {}
            };
            Ok((input, sql, entity_match, is_empty_check, no_spatial_check))
        }
        _ => Err(syn::Error::new(
            field_name.span(),
            format!("Unsupported filter type: {}", filter_type),
        )),
    }
}

// ============================================================================
// Row Field Assignment Generation
// ============================================================================

fn generate_row_field_assignment(
    backend: BackendKind,
    field_name: &syn::Ident,
    field_type: &syn::Type,
    db_col: &str,
    meta: &FieldMetadata,
) -> syn::Result<proc_macro2::TokenStream> {
    let bool_expr = if backend == BackendKind::Postgres || backend == BackendKind::Mssql {
        quote! { row.try_get::<bool, _>(#db_col)? }
    } else {
        quote! {{
            let i: i32 = row.try_get(#db_col)?;
            int_to_bool(i)
        }}
    };
    let uuid_expr = if backend == BackendKind::Postgres || backend == BackendKind::Mssql {
        quote! { row.try_get(#db_col)? }
    } else {
        quote! {{
            let s: String = row.try_get(#db_col)?;
            str_to_uuid(&s).map_err(|e| ::graphql_orm::sqlx::Error::Decode(e.into()))?
        }}
    };
    let datetime_expr = if backend == BackendKind::Postgres {
        quote! { row.try_get(#db_col)? }
    } else {
        quote! {{
            let s: String = row.try_get(#db_col)?;
            str_to_datetime(&s).map_err(|e| ::graphql_orm::sqlx::Error::Decode(e.into()))?
        }}
    };
    // Handle special field types
    if meta.is_date_field {
        return Ok(quote! {
            #field_name: {
                let s: Option<String> = row.try_get(#db_col)?;
                s.and_then(|s| str_to_datetime(&s).ok())
            },
        });
    }
    if meta.is_boolean_field {
        return Ok(quote! {
            #field_name: #bool_expr,
        });
    }
    if meta.decimal.is_some() {
        if let Some(inner_type) = option_inner_type(field_type) {
            return generate_option_row_field_assignment(
                backend, field_name, inner_type, db_col, meta,
            );
        }
        let definition = decimal_definition_tokens(meta);
        let value = match backend {
            BackendKind::Sqlite => quote! {{
                let value: i64 = row.try_get(#db_col)?;
                #definition.from_scaled_i64(value)
            }},
            BackendKind::Postgres => quote! { row.try_get(#db_col)? },
            BackendKind::Mssql if meta.mssql_money => quote! {{
                let value: ::graphql_orm::rust_decimal::Decimal = row.try_get(#db_col)?;
                #definition.normalize(value).map_err(|error| ::graphql_orm::sqlx::Error::ColumnDecode {
                    index: #db_col.to_string(),
                    source: Box::new(error),
                })?
            }},
            BackendKind::Mssql => quote! { row.try_get(#db_col)? },
        };
        return Ok(quote! { #field_name: #value, });
    }

    if meta.spatial.is_some() {
        if is_option_type(field_type) {
            return Ok(quote! {
                #field_name: {
                    let s: Option<String> = row.try_get(#db_col)?;
                    s.map(|value| ::graphql_orm::serde_json::from_str::<::graphql_orm::serde_json::Value>(&value))
                        .transpose()
                        .map_err(|e| ::graphql_orm::sqlx::Error::Decode(e.into()))?
                },
            });
        }
        return Ok(quote! {
            #field_name: {
                let s: String = row.try_get(#db_col)?;
                ::graphql_orm::serde_json::from_str::<::graphql_orm::serde_json::Value>(&s)
                    .map_err(|e| ::graphql_orm::sqlx::Error::Decode(e.into()))?
            },
        });
    }

    if meta.is_json_field && !is_option_type(field_type) {
        let json_expr = if backend == BackendKind::Postgres {
            quote! {{
                let value: ::graphql_orm::sqlx::types::Json<::graphql_orm::serde_json::Value> =
                    row.try_get(#db_col)?;
                json_from_value(value.0)?
            }}
        } else {
            quote! {{
                let s: String = row.try_get(#db_col)?;
                json_from_str(&s)?
            }}
        };
        return Ok(quote! {
            #field_name: #json_expr,
        });
    }

    // Check type and generate appropriate code
    if let syn::Type::Path(type_path) = field_type {
        if let Some(segment) = type_path.path.segments.last() {
            let type_name = segment.ident.to_string();

            match type_name.as_str() {
                "String" => {
                    let raw_expr = quote! { row.try_get::<String, _>(#db_col)? };
                    let expr = maybe_wrap_read_transform(raw_expr, &meta.transform_read);
                    return Ok(quote! {
                        #field_name: #expr,
                    });
                }
                "i32" | "i64" => {
                    return Ok(quote! {
                        #field_name: row.try_get(#db_col)?,
                    });
                }
                "f32" | "f64" => {
                    return Ok(quote! {
                        #field_name: row.try_get(#db_col)?,
                    });
                }
                "Decimal" => unreachable!("Decimal fields require validated decimal metadata"),
                "bool" => {
                    return Ok(quote! {
                        #field_name: #bool_expr,
                    });
                }
                "Uuid" => {
                    return Ok(quote! {
                        #field_name: #uuid_expr,
                    });
                }
                "Option" => {
                    if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                        if let Some(syn::GenericArgument::Type(inner_type)) = args.args.first() {
                            return generate_option_row_field_assignment(
                                backend, field_name, inner_type, db_col, meta,
                            );
                        }
                    }
                }
                "Vec" => {
                    if is_byte_vec_type(field_type) {
                        return Ok(quote! {
                            #field_name: row.try_get(#db_col)?,
                        });
                    }
                    // JSON array field
                    return Ok(quote! {
                        #field_name: {
                            let s: String = row.try_get(#db_col)?;
                            json_to_vec(&s)
                        },
                    });
                }
                "DateTime" => {
                    return Ok(quote! {
                        #field_name: #datetime_expr,
                    });
                }
                _ => {}
            }
        }
    }

    // Default: try direct get
    Ok(quote! {
        #field_name: row.try_get(#db_col)?,
    })
}

fn generate_option_row_field_assignment(
    backend: BackendKind,
    field_name: &syn::Ident,
    inner_type: &syn::Type,
    db_col: &str,
    meta: &FieldMetadata,
) -> syn::Result<proc_macro2::TokenStream> {
    let optional_bool_expr = if backend == BackendKind::Postgres || backend == BackendKind::Mssql {
        quote! { row.try_get(#db_col)? }
    } else {
        quote! {{
            let i: Option<i32> = row.try_get(#db_col)?;
            i.map(int_to_bool)
        }}
    };
    let optional_uuid_expr = if backend == BackendKind::Postgres || backend == BackendKind::Mssql {
        quote! { row.try_get(#db_col)? }
    } else {
        quote! {{
            let s: Option<String> = row.try_get(#db_col)?;
            s.map(|s| str_to_uuid(&s))
                .transpose()
                .map_err(|e| ::graphql_orm::sqlx::Error::Decode(e.into()))?
        }}
    };
    let optional_datetime_expr = if backend == BackendKind::Postgres {
        quote! { row.try_get(#db_col)? }
    } else {
        quote! {{
            let s: Option<String> = row.try_get(#db_col)?;
            s.map(|s| str_to_datetime(&s))
                .transpose()
                .map_err(|e| ::graphql_orm::sqlx::Error::Decode(e.into()))?
        }}
    };
    if meta.decimal.is_some() {
        let definition = decimal_definition_tokens(meta);
        let value = match backend {
            BackendKind::Sqlite => quote! {{
                let value: Option<i64> = row.try_get(#db_col)?;
                value.map(|value| #definition.from_scaled_i64(value))
            }},
            BackendKind::Postgres => quote! { row.try_get(#db_col)? },
            BackendKind::Mssql if meta.mssql_money => quote! {{
                let value: Option<::graphql_orm::rust_decimal::Decimal> = row.try_get(#db_col)?;
                value.map(|value| #definition.normalize(value))
                    .transpose()
                    .map_err(|error| ::graphql_orm::sqlx::Error::ColumnDecode {
                        index: #db_col.to_string(),
                        source: Box::new(error),
                    })?
            }},
            BackendKind::Mssql => quote! { row.try_get(#db_col)? },
        };
        return Ok(quote! { #field_name: #value, });
    }
    if let syn::Type::Path(inner_path) = inner_type {
        if let Some(segment) = inner_path.path.segments.last() {
            let inner_name = segment.ident.to_string();

            if meta.is_json_field {
                let json_expr = if backend == BackendKind::Postgres {
                    quote! {{
                        let value: Option<::graphql_orm::sqlx::types::Json<::graphql_orm::serde_json::Value>> =
                            row.try_get(#db_col)?;
                        value.map(|json| json_from_value(json.0)).transpose()?
                    }}
                } else {
                    quote! {{
                        let s: Option<String> = row.try_get(#db_col)?;
                        match s.as_deref() {
                            Some("") | None => None,
                            Some(value) => Some(json_from_str(value)?),
                        }
                    }}
                };
                return Ok(quote! {
                    #field_name: #json_expr,
                });
            }

            match inner_name.as_str() {
                "String" => {
                    if let Some(transform_read) = &meta.transform_read {
                        let transform_path: syn::Path = syn::parse_str(transform_read)
                            .unwrap_or_else(|_| syn::parse_str("unknown_transform").unwrap());
                        return Ok(quote! {
                            #field_name: {
                                let v: Option<String> = row.try_get(#db_col)?;
                                v.map(|s| #transform_path(s))
                            },
                        });
                    }
                    return Ok(quote! {
                        #field_name: row.try_get(#db_col)?,
                    });
                }
                "i32" | "i64" => {
                    return Ok(quote! {
                        #field_name: row.try_get(#db_col)?,
                    });
                }
                "f32" | "f64" => {
                    return Ok(quote! {
                        #field_name: row.try_get(#db_col)?,
                    });
                }
                "bool" => {
                    return Ok(quote! {
                        #field_name: #optional_bool_expr,
                    });
                }
                "Uuid" => {
                    return Ok(quote! {
                        #field_name: #optional_uuid_expr,
                    });
                }
                "DateTime" => {
                    return Ok(quote! {
                        #field_name: #optional_datetime_expr,
                    });
                }
                "Vec" => {
                    if is_byte_vec_type(inner_type) {
                        return Ok(quote! {
                            #field_name: row.try_get(#db_col)?,
                        });
                    }
                    return Ok(quote! {
                        #field_name: {
                            let s: Option<String> = row.try_get(#db_col)?;
                            s.map(|s| json_to_vec(&s))
                        },
                    });
                }
                _ => {}
            }
        }
    }

    Ok(quote! {
        #field_name: row.try_get(#db_col)?,
    })
}

pub(crate) fn is_vec_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            return segment.ident == "Vec";
        }
    }
    false
}

pub(crate) fn is_byte_vec_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident != "Vec" {
                return false;
            }

            if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                if let Some(syn::GenericArgument::Type(syn::Type::Path(inner_path))) =
                    args.args.first()
                {
                    return inner_path
                        .path
                        .segments
                        .last()
                        .is_some_and(|inner| inner.ident == "u8");
                }
            }
        }
    }

    false
}

pub(crate) fn is_option_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            return segment.ident == "Option";
        }
    }
    false
}

pub(crate) fn is_bool_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "bool" {
                return true;
            }
            // Check for Option<bool>
            if segment.ident == "Option" {
                if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return is_bool_type(inner);
                    }
                }
            }
        }
    }
    false
}

pub(crate) fn is_string_type(ty: &syn::Type) -> bool {
    if type_path_last_ident(ty).is_some_and(|ident| ident == "String") {
        return true;
    }

    option_inner_type(ty).is_some_and(is_string_type)
}

pub(crate) fn type_path_last_ident(ty: &syn::Type) -> Option<&syn::Ident> {
    if let syn::Type::Path(type_path) = ty {
        return type_path.path.segments.last().map(|segment| &segment.ident);
    }

    None
}

pub(crate) fn option_inner_type(ty: &syn::Type) -> Option<&syn::Type> {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "Option" {
                if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return Some(inner);
                    }
                }
            }
        }
    }

    None
}

pub(crate) fn is_uuid_type(ty: &syn::Type) -> bool {
    if type_path_last_ident(ty).is_some_and(|ident| ident == "Uuid") {
        return true;
    }

    option_inner_type(ty).is_some_and(is_uuid_type)
}

fn is_serde_json_value_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(type_path) = ty {
        let segments = type_path
            .path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect::<Vec<_>>();
        return segments.last().is_some_and(|ident| ident == "Value")
            && (segments.len() == 1
                || segments
                    .windows(2)
                    .any(|window| window[0] == "serde_json" && window[1] == "Value")
                || segments.windows(3).any(|window| {
                    window[0] == "graphql_orm" && window[1] == "serde_json" && window[2] == "Value"
                }));
    }

    false
}

pub(crate) fn is_serde_json_value_or_option(ty: &syn::Type) -> bool {
    is_serde_json_value_type(ty) || option_inner_type(ty).is_some_and(is_serde_json_value_type)
}

/// Convert Rust type to the configured backend SQL type string
pub(crate) fn rust_type_to_sql_type(
    backend: BackendKind,
    ty: &syn::Type,
    meta: &FieldMetadata,
) -> String {
    // Handle Option<T> by unwrapping
    let inner_type = if is_option_type(ty) {
        if let syn::Type::Path(type_path) = ty {
            if let Some(segment) = type_path.path.segments.last() {
                if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        inner
                    } else {
                        ty
                    }
                } else {
                    ty
                }
            } else {
                ty
            }
        } else {
            ty
        }
    } else {
        ty
    };

    // Check field metadata first
    if let Some(spatial) = &meta.spatial {
        return match backend {
            BackendKind::Postgres => {
                format!("geometry({},{})", spatial.geometry_type, spatial.srid)
            }
            BackendKind::Sqlite => "TEXT".to_string(),
            BackendKind::Mssql => "NVARCHAR(MAX)".to_string(),
        };
    }
    if meta.is_boolean_field {
        return match backend {
            BackendKind::Postgres => "BOOLEAN",
            BackendKind::Mssql => "BIT",
            BackendKind::Sqlite => "INTEGER",
        }
        .to_string();
    }
    if meta.is_json_field {
        return match backend {
            BackendKind::Postgres => "JSONB",
            BackendKind::Mssql => "NVARCHAR(MAX)",
            BackendKind::Sqlite => "TEXT",
        }
        .to_string();
    }
    if meta.is_date_field {
        return match backend {
            BackendKind::Postgres => "TIMESTAMPTZ",
            BackendKind::Mssql => "DATETIME2",
            BackendKind::Sqlite => "TEXT",
        }
        .to_string();
    }
    if meta.mssql_money {
        return "MONEY".to_string();
    }
    if let Some((precision, scale)) = meta.decimal {
        return match backend {
            BackendKind::Sqlite => "INTEGER".to_string(),
            BackendKind::Postgres => format!("NUMERIC({precision},{scale})"),
            BackendKind::Mssql => format!("DECIMAL({precision},{scale})"),
        };
    }

    // Infer from Rust type first (so f64 becomes REAL not INTEGER for "number" filter)
    if let syn::Type::Path(type_path) = inner_type {
        if let Some(segment) = type_path.path.segments.last() {
            let type_name = segment.ident.to_string();
            match type_name.as_str() {
                "String" | "str" => {
                    return if backend == BackendKind::Mssql {
                        "NVARCHAR(MAX)"
                    } else {
                        "TEXT"
                    }
                    .to_string();
                }
                "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64" | "usize" => {
                    return match backend {
                        BackendKind::Postgres => "BIGINT",
                        BackendKind::Mssql => "BIGINT",
                        BackendKind::Sqlite => "INTEGER",
                    }
                    .to_string();
                }
                "f32" | "f64" => {
                    return match backend {
                        BackendKind::Postgres => "DOUBLE PRECISION",
                        BackendKind::Mssql => "FLOAT",
                        BackendKind::Sqlite => "REAL",
                    }
                    .to_string();
                }
                "bool" => {
                    return match backend {
                        BackendKind::Postgres => "BOOLEAN",
                        BackendKind::Mssql => "BIT",
                        BackendKind::Sqlite => "INTEGER",
                    }
                    .to_string();
                }
                "Vec" => {
                    if is_byte_vec_type(inner_type) {
                        return match backend {
                            BackendKind::Postgres => "BYTEA",
                            BackendKind::Mssql => "VARBINARY(MAX)",
                            BackendKind::Sqlite => "BLOB",
                        }
                        .to_string();
                    }
                    return match backend {
                        BackendKind::Postgres => "JSONB",
                        BackendKind::Mssql => "NVARCHAR(MAX)",
                        BackendKind::Sqlite => "TEXT",
                    }
                    .to_string();
                }
                "Uuid" => {
                    return match backend {
                        BackendKind::Postgres => "UUID",
                        BackendKind::Mssql => "UNIQUEIDENTIFIER",
                        BackendKind::Sqlite => "TEXT",
                    }
                    .to_string();
                }
                "DateTime" => {
                    return match backend {
                        BackendKind::Postgres => "TIMESTAMPTZ",
                        BackendKind::Mssql => "DATETIME2",
                        BackendKind::Sqlite => "TEXT",
                    }
                    .to_string();
                }
                _ => return "TEXT".to_string(),
            }
        }
    }

    "TEXT".to_string()
}

fn decimal_default_sql(
    backend: BackendKind,
    value: &str,
    precision: u8,
    scale: u8,
    span: proc_macro2::Span,
) -> syn::Result<String> {
    use std::str::FromStr;

    let parsed = rust_decimal::Decimal::from_str(value)
        .map_err(|_| syn::Error::new(span, "decimal defaults must be exact decimal literals"))?;
    let mut normalized = parsed;
    normalized.rescale(u32::from(scale));
    if normalized != parsed
        || normalized.mantissa().unsigned_abs().to_string().len() > usize::from(precision)
        || (backend == BackendKind::Mssql
            && precision == 19
            && scale == 4
            && i64::try_from(normalized.mantissa()).is_err())
    {
        return Err(syn::Error::new(
            span,
            "decimal default exceeds its declared precision or scale",
        ));
    }
    match backend {
        BackendKind::Sqlite => i64::try_from(normalized.mantissa())
            .map(|value| value.to_string())
            .map_err(|_| syn::Error::new(span, "decimal default exceeds SQLite's exact range")),
        BackendKind::Postgres | BackendKind::Mssql => Ok(normalized.to_string()),
    }
}

#[cfg(test)]
mod host_timestamp_tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn classification_preserves_distinct_legacy_name_rules() {
        let rust_timestamp: Field = parse_quote!(created_at: i64);
        let physical_timestamp: Field =
            parse_quote!(#[graphql_orm(db_column = "updated_at")] modified: i64);
        let meta = parse_field_metadata(&rust_timestamp).unwrap();
        assert!(meta.timestamp_behavior("created_at").exclude_input);
        assert!(!meta.timestamp_behavior("created_at").update_updated_at);
        let meta = parse_field_metadata(&physical_timestamp).unwrap();
        assert!(!meta.timestamp_behavior("modified").exclude_input);
        assert!(meta.timestamp_behavior("modified").update_updated_at);
        let host: Field = parse_quote!(#[graphql_orm(timestamp = "host", db_column = "updated_at")] created_at: i64);
        let meta = parse_field_metadata(&host).unwrap();
        assert!(!meta.timestamp_behavior("created_at").exclude_input);
        assert!(!meta.timestamp_behavior("created_at").update_updated_at);
    }

    #[test]
    fn unsupported_modes_and_duplicate_annotations_fail() {
        for field in [
            parse_quote!(#[graphql_orm(timestamp = "automatic")] created_at: i64),
            parse_quote!(#[graphql_orm(timestamp = "host", timestamp = "host")] created_at: i64),
            parse_quote!(#[graphql_orm(timestamp = "host")] #[graphql_orm(timestamp = "host")] created_at: i64),
        ] {
            assert!(parse_field_metadata(&field).is_err());
        }
    }

    #[test]
    fn invalid_declarations_fail_before_emission() {
        let invalid: Vec<Field> = vec![
            parse_quote!(#[graphql_orm(timestamp = "host")] unrelated: i64),
            parse_quote!(#[graphql_orm(timestamp = "host")] created_at: bool),
            parse_quote!(#[graphql_orm(timestamp = "host")] created_at: Option<Option<i64>>),
            parse_quote!(#[graphql_orm(timestamp = "host")] #[date_field] created_at: i64),
            parse_quote!(#[graphql_orm(timestamp = "host")] #[skip_db] created_at: i64),
            parse_quote!(#[graphql_orm(timestamp = "host")] #[primary_key] created_at: i64),
            parse_quote!(#[graphql_orm(timestamp = "host", version)] updated_at: i64),
            parse_quote!(#[graphql_orm(timestamp = "host", auto_generated = true)] created_at: i64),
            parse_quote!(#[graphql_orm(timestamp = "host")] #[relation(target = "Other", from = "id", to = "id")] created_at: Option<i64>),
        ];
        for field in invalid {
            assert!(validate_host_timestamp(&field, true, false).is_err());
        }
        let valid: Field = parse_quote!(#[graphql_orm(timestamp = "host", db_column = "updated_at", default = false, auto_generated = false)] modified: Option<i64>);
        assert!(validate_host_timestamp(&valid, true, false).is_ok());
        let text: Field = parse_quote!(#[graphql_orm(timestamp = "host", db_column = "updated_at")] observed: Option<String>);
        assert!(validate_host_timestamp(&text, true, false).is_ok());
        assert!(validate_host_timestamp(&valid, false, false).is_err());
        let restricted: Field =
            parse_quote!(#[graphql_orm(timestamp = "host", write = false)] created_at: i64);
        assert!(validate_host_timestamp(&restricted, true, false).is_ok());
        let inferred: Field = parse_quote!(#[graphql_orm(timestamp = "host", db_column = "updated_at", auto_generated = false)] id: i64);
        assert!(validate_host_timestamp(&inferred, true, true).is_err());
        assert!(validate_host_timestamp(&inferred, true, false).is_ok());
        let fields = syn::punctuated::Punctuated::from_iter([inferred.clone()]);
        assert!(validate_host_timestamp_fields(&fields, true).is_err());
        let explicit: Field = parse_quote!(#[primary_key] key: String);
        let fields = syn::punctuated::Punctuated::from_iter([explicit, inferred]);
        assert!(validate_host_timestamp_fields(&fields, true).is_ok());
    }
}

#[cfg(test)]
mod mssql_money_attribute_tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn rejects_wrong_backend_and_conflicting_decimal_metadata() {
        let wrong_backend: DeriveInput = parse_quote! {
            #[graphql_entity(backend = "sqlite", table = "amounts", plural = "Amounts")]
            struct Amount { #[primary_key] id: i64, #[graphql_orm(mssql_money)] value: rust_decimal::Decimal }
        };
        assert!(
            generate_graphql_entity(&wrong_backend)
                .unwrap_err()
                .to_string()
                .contains("mssql_money requires backend")
        );
        let conflicting: Field = parse_quote! {
            #[graphql_orm(mssql_money, decimal(precision = 18, scale = 4))]
            value: rust_decimal::Decimal
        };
        assert!(
            parse_field_metadata(&conflicting)
                .err()
                .unwrap()
                .to_string()
                .contains("cannot be combined")
        );
        let valid: Field = parse_quote! {
            #[graphql_orm(mssql_money)] value: Option<rust_decimal::Decimal>
        };
        let parsed = parse_field_metadata(&valid).unwrap();
        assert_eq!(parsed.decimal, Some((19, 4)));
        assert!(parsed.mssql_money);
        let wrong_type: Field = parse_quote! {
            #[graphql_orm(mssql_money)] value: String
        };
        assert!(
            parse_field_metadata(&wrong_type)
                .err()
                .unwrap()
                .to_string()
                .contains("requires rust_decimal::Decimal")
        );
    }
}

#[cfg(test)]
mod generated_identifier_tests {
    use super::bounded_generated_identifier;

    #[test]
    fn names_within_the_limit_are_unchanged() {
        let exact = format!("idx_{}", "a".repeat(59));
        assert_eq!(exact.len(), 63);
        assert_eq!(bounded_generated_identifier(exact.clone()), exact);
        assert_eq!(
            bounded_generated_identifier("idx_users_email".to_owned()),
            "idx_users_email"
        );
    }

    #[test]
    fn long_names_are_deterministically_bounded() {
        let full = "idx_graphql_orm_ai_provider_webhook_receipts_provider_kind_provider_profile_id_provider_response_id_state".to_owned();
        let bounded = bounded_generated_identifier(full.clone());
        assert_eq!(bounded.len(), 63);
        assert!(bounded.starts_with("idx_graphql_orm_ai_provider_webhook_receipts_p_"));
        assert_eq!(bounded, bounded_generated_identifier(full.clone()));
        let other = bounded_generated_identifier(format!("{full}_x"));
        assert_ne!(bounded, other);
        assert_eq!(&bounded[..46], &other[..46]);
    }

    #[test]
    fn bounding_respects_character_boundaries() {
        let full = format!("idx_{}é{}", "a".repeat(41), "b".repeat(40));
        let bounded = bounded_generated_identifier(full);
        assert!(bounded.len() <= 63);
        assert!(bounded.is_char_boundary(bounded.len() - 17));
    }
}

#[cfg(test)]
mod object_visibility_tests {
    use super::*;
    use syn::parse::Parser;

    #[test]
    fn unreadable_relation_keys_and_changed_keys_are_rejected_on_every_backend() {
        for backend in crate::backend::enabled_backends() {
            let backend = backend.name();
            for attribute in [
                "#[graphql_orm(private)]",
                "#[graphql_orm(read = false)]",
                "#[input_only]",
            ] {
                let input: DeriveInput = syn::parse_str(&format!(r#"
                    #[graphql_entity(backend = "{backend}", table = "records", plural = "Records")]
                    struct Record {{
                        #[primary_key] id: String,
                        {attribute} hidden_key: String,
                        #[relation(target = "Record", from = "hidden_key", to = "id", emit_fk = false)]
                        related: Vec<Record>,
                    }}
                "#)).unwrap();
                let error = generate_entity_impl(&input, false, false).unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains("unreadable local key `hidden_key`")
                );
                // Hiding the relation is a supported repair on every backend.
                let mut repaired = input.clone();
                let syn::Data::Struct(data) = &mut repaired.data else {
                    unreachable!()
                };
                data.fields
                    .iter_mut()
                    .next_back()
                    .unwrap()
                    .attrs
                    .push(syn::parse_quote!(#[graphql_orm(read = false)]));
                generate_entity_impl(&repaired, false, false).unwrap();

                let input: DeriveInput = syn::parse_str(&format!(
                    r#"
                    #[graphql_entity(backend = "{backend}", table = "records", plural = "Records")]
                    struct Record {{
                        #[primary_key] {attribute} id: String,
                        label: String,
                    }}
                "#
                ))
                .unwrap();
                let error = crate::operations::generate_graphql_operations(&input).unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains("generated Changed events disclose primary keys")
                );
            }
        }
    }

    #[test]
    fn condition_columns_require_readability_on_every_backend() {
        for backend in crate::backend::enabled_backends() {
            for attribute in [
                "#[graphql_orm(private)]",
                "#[graphql_orm(read = false)]",
                "#[input_only]",
            ] {
                for (condition, message) in [
                    (
                        "source_condition(field = \"tier\", equals = 7)",
                        "unreadable source condition column `tier`",
                    ),
                    (
                        "target_condition(column = \"tier_code\", equals = 7)",
                        "unreadable self-target condition column `tier_code`",
                    ),
                ] {
                    let input: DeriveInput = syn::parse_str(&format!(r#"
                        #[graphql_entity(backend = "{backend}", table = "records", plural = "Records")]
                        struct Record {{
                            #[primary_key] id: i32,
                            {attribute} #[graphql_orm(db_column = "tier_code")] tier: i32,
                            #[relation(target = "Record", from = "id", to = "id", {condition}, multiple, emit_fk = false)]
                            related: Vec<Record>,
                        }}
                    "#, backend = backend.name())).unwrap();
                    assert!(
                        generate_entity_impl(&input, false, false)
                            .unwrap_err()
                            .to_string()
                            .contains(message)
                    );
                    let mut hidden = input.clone();
                    let syn::Data::Struct(data) = &mut hidden.data else {
                        unreachable!()
                    };
                    data.fields
                        .iter_mut()
                        .next_back()
                        .unwrap()
                        .attrs
                        .push(syn::parse_quote!(#[graphql_orm(read = false)]));
                    generate_entity_impl(&hidden, false, false).unwrap();
                }
            }
        }
    }

    #[test]
    fn self_relation_keys_resolve_physical_names_and_sql_quoting() {
        for backend in crate::backend::enabled_backends() {
            for target in ["SecretKey", "secretkey", "\"SecretKey\""] {
                let public_column = if backend == BackendKind::Postgres && target == "secretkey" {
                    "#[graphql_orm(db_column = \"secretkey\")] public_key: String,"
                } else {
                    ""
                };
                let input: DeriveInput = syn::parse_str(&format!(r##"
                    #[graphql_entity(backend = "{backend}", table = "records", plural = "Records")]
                    struct Record {{
                        #[primary_key] id: String,
                        #[graphql_orm(private, db_column = "SecretKey")]
                        hidden_key: String,
                        {public_column}
                        #[relation(target = "Record", from = "id", to = {target:?}, emit_fk = false)]
                        related: Vec<Record>,
                    }}
                "##, backend=backend.name())).unwrap();
                let result = generate_entity_impl(&input, false, false);
                if backend == BackendKind::Postgres && target == "secretkey" {
                    result.unwrap();
                } else {
                    assert!(result.unwrap_err().to_string().contains("self-target key"));
                }
            }
        }
    }

    #[test]
    fn relation_output_metadata_distinguishes_owned_implementation_skips() {
        for backend in crate::backend::enabled_backends() {
            for (attribute, delegated) in [
                ("", true),
                ("#[graphql(skip)]", false),
                ("#[graphql(skip = true)]", false),
                ("#[graphql(skip = false)]", true),
                ("#[graphql(skip_output)]", false),
                ("#[graphql(skip_output = false)]", true),
                ("#[graphql(visible = false)]", false),
                ("#[graphql(guard = \"Guard\")]", false),
                ("#[graphql_orm(private)]", false),
                ("#[graphql_orm(read = false)]", false),
                ("#[input_only]", false),
                ("#[graphql_orm(read_policy = \"owner\")]", false),
            ] {
                for metadata_only in [false, true] {
                    let input: DeriveInput = syn::parse_str(&format!(r#"
                        #[graphql_entity(backend = "{backend}", table = "records", plural = "Records")]
                        struct Record {{
                            #[primary_key] id: i64,
                            {attribute}
                            #[relation(target = "Record", from = "id", to = "id", emit_fk = false)]
                            related: Option<Box<Record>>,
                        }}
                    "#, backend = backend.name())).unwrap();
                    let tokens = generate_entity_impl(&input, metadata_only, false)
                        .unwrap()
                        .to_string();
                    let relation = tokens.split("RelationMetadata {").nth(1).unwrap();
                    let expected = metadata_only && delegated;
                    assert!(
                        relation
                            .split("is_readable : ")
                            .nth(1)
                            .unwrap()
                            .starts_with(&format!("{expected} ||")),
                        "{attribute}, metadata_only={metadata_only}: {relation}"
                    );
                }
            }
        }
    }

    #[test]
    fn scalar_output_metadata_follows_ownership_and_effective_flags() {
        for (attribute, delegated_readable) in [
            ("", true),
            ("#[graphql(skip)]", false),
            ("#[graphql(skip = true)]", false),
            ("#[graphql(skip = false)]", true),
            ("#[graphql(skip_output)]", false),
            ("#[graphql(skip_output = true)]", false),
            ("#[graphql(skip_output = false)]", true),
            ("#[graphql(flatten)]", false),
            ("#[graphql(flatten = false)]", true),
            ("#[graphql(visible = false)]", false),
            ("#[graphql(visible = true)]", true),
            ("#[graphql(visible = \"visible_to_reader\")]", false),
            ("#[graphql(guard = \"Guard\")]", false),
            ("#[graphql(inaccessible)]", false),
            ("#[graphql(inaccessible = false)]", true),
            (
                "#[graphql(derived(name = \"alias\", into = \"String\"))]",
                true,
            ),
            (
                "#[graphql(skip, derived(name = \"alias\", into = \"String\"))]",
                false,
            ),
            ("#[serde(skip_serializing)]", true),
            ("#[graphql(skip_input, secret)]", true),
        ] {
            let field = syn::Field::parse_named
                .parse_str(&format!("{attribute} value: String"))
                .unwrap();
            let metadata = parse_field_metadata(&field).unwrap();
            assert_eq!(
                scalar_output_is_readable(&field, &metadata, false, &[]),
                delegated_readable,
                "{attribute}"
            );
            assert!(
                scalar_output_is_readable(&field, &metadata, true, &[]),
                "{attribute}"
            );
            // This shared predicate also describes relation implementation fields.
            assert!(metadata.graphql_readable(), "{attribute}");
        }
        let field: syn::Field = syn::parse_quote!(value: String);
        let metadata = parse_field_metadata(&field).unwrap();
        for object in [
            syn::parse_quote!(#[graphql(visible = false)]),
            syn::parse_quote!(#[graphql(visible = "visible_to_reader")]),
            syn::parse_quote!(#[graphql(guard = "Guard")]),
        ] {
            assert!(!scalar_output_is_readable(
                &field,
                &metadata,
                false,
                &[object]
            ));
        }
        for field in [
            syn::parse_quote!(#[graphql_orm(private)] value: String),
            syn::parse_quote!(#[graphql_orm(read = false)] value: String),
            syn::parse_quote!(#[input_only] value: String),
            syn::parse_quote!(#[graphql_orm(read_policy = "policy")] value: String),
        ] {
            let metadata = parse_field_metadata(&field).unwrap();
            for owns_output in [false, true] {
                assert!(!scalar_output_is_readable(
                    &field,
                    &metadata,
                    owns_output,
                    &[]
                ));
            }
        }
    }

    #[test]
    fn only_effective_object_skips_satisfy_visibility() {
        for (attribute, skipped) in [
            ("#[graphql(skip)]", true),
            ("#[graphql(skip = true)]", true),
            ("#[graphql(skip = false)]", false),
            ("#[graphql(visible = false, skip)]", true),
            ("#[graphql(visible = false)]", false),
        ] {
            let field: syn::Field = syn::Field::parse_named
                .parse_str(&format!("{attribute} hidden: String"))
                .unwrap();
            assert_eq!(has_graphql_object_skip(&field), skipped);
        }
    }

    #[test]
    fn unreadable_simple_object_fields_require_skips() {
        for backend in crate::backend::enabled_backends() {
            let backend = backend.name();
            for (object_attrs, mode) in [
                ("#[graphql(complex)]", ""),
                ("", ", schema_only = true"),
                ("#[graphql(complex)]", ", schema_only = true"),
            ] {
                for attribute in [
                    "#[graphql_orm(private)]",
                    "#[graphql_orm(read = false)]",
                    "#[input_only]",
                ] {
                    let input: DeriveInput = syn::parse_str(&format!(
                    r#"
                    {object_attrs}
                    #[graphql_entity(backend = "{backend}", table = "records", plural = "Records" {mode})]
                    struct Record {{
                        #[primary_key]
                        #[filterable(type = "string")]
                        #[sortable]
                        id: String,
                        label: String,
                        {attribute}
                        hidden: String,
                    }}
                "#
                ))
                .unwrap();
                    let error = generate_entity_impl(&input, false, false).unwrap_err();
                    assert!(error.to_string().contains("unreadable SimpleObject field"));
                }
            }
        }
    }
}

#[cfg(test)]
mod default_sort_tests {
    use super::*;
    #[cfg(feature = "sqlite")]
    use syn::parse_quote;

    #[test]
    fn primary_key_default_sort_renders_every_key_column_per_backend() {
        assert_eq!(
            primary_key_default_sort(BackendKind::Sqlite, &["region".into(), "slot".into()]),
            "region, slot"
        );
        // Reserved or spaced physical names stay valid SQL.
        assert_eq!(
            primary_key_default_sort(BackendKind::Postgres, &["order".into(), "line no".into()]),
            "\"order\", \"line no\""
        );
        // MSSQL key identities are already bracketed and are not re-quoted.
        let keys =
            ["region", "slot"].map(|key| backend_quote_identifier_path(BackendKind::Mssql, key));
        assert_eq!(
            primary_key_default_sort(BackendKind::Mssql, &keys),
            "[region], [slot]"
        );
        assert_eq!(
            primary_key_default_sort(
                BackendKind::Mssql,
                &[backend_quote_identifier_path(BackendKind::Mssql, "id")]
            ),
            "[id]"
        );
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn id_default_expansion_is_byte_identical_to_explicit_id() {
        let implicit: DeriveInput = parse_quote! {
            #[graphql_entity(backend = "sqlite", table = "records")]
            struct Record {
                #[primary_key]
                #[sortable]
                id: String,
            }
        };
        let explicit: DeriveInput = parse_quote! {
            #[graphql_entity(backend = "sqlite", table = "records", default_sort = "id")]
            struct Record {
                #[primary_key]
                #[sortable]
                id: String,
            }
        };
        assert_eq!(
            generate_graphql_entity(&implicit).unwrap().to_string(),
            generate_graphql_entity(&explicit).unwrap().to_string(),
        );
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn implicit_id_key_uses_its_physical_column() {
        let input: DeriveInput = parse_quote! {
            #[graphql_entity(backend = "sqlite", table = "records")]
            struct Record {
                #[graphql_orm(db_column = "record_key")]
                id: String,
                label: String,
            }
        };
        let generated = generate_graphql_entity(&input).unwrap().to_string();
        assert!(
            generated.contains("const DEFAULT_SORT : & 'static str = \"record_key\""),
            "{generated}"
        );
        assert!(
            generated
                .contains("const PRIMARY_KEYS : & 'static [& 'static str] = & [\"record_key\"]"),
            "{generated}"
        );
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn composite_key_default_follows_declaration_order() {
        let input: DeriveInput = parse_quote! {
            #[graphql_entity(backend = "sqlite", table = "allocations")]
            struct Allocation {
                #[primary_key]
                #[graphql_orm(db_column = "slot_number")]
                slot: i64,
                #[primary_key]
                region: String,
            }
        };
        let generated = generate_graphql_entity(&input).unwrap().to_string();
        assert!(
            generated.contains("const DEFAULT_SORT : & 'static str = \"slot_number, region\""),
            "{generated}"
        );
    }
}

#[cfg(test)]
mod attribute_composition_tests {
    use super::*;
    use syn::parse::Parser;
    use syn::parse_quote;

    #[test]
    fn complex_is_found_in_any_option_order() {
        for attrs in [
            vec![parse_quote!(#[graphql(rename_fields = "PascalCase", complex)])],
            vec![parse_quote!(#[graphql(complex, rename_fields = "PascalCase")])],
            vec![
                parse_quote!(#[graphql(rename_fields = "PascalCase")]),
                parse_quote!(#[graphql(complex)]),
            ],
            vec![
                parse_quote!(#[graphql(guard = "Guard::new()", concrete(name = "A", params(i32)), complex)]),
            ],
        ] {
            let attrs: Vec<syn::Attribute> = attrs;
            assert!(has_graphql_complex(&attrs).unwrap());
        }
        let plain: Vec<syn::Attribute> =
            vec![parse_quote!(#[graphql(rename_fields = "PascalCase")])];
        assert!(!has_graphql_complex(&plain).unwrap());
        let malformed: Vec<syn::Attribute> =
            vec![parse_quote!(#[graphql(rename_fields = "PascalCase" complex)])];
        assert!(has_graphql_complex(&malformed).is_err());
    }

    #[test]
    fn entity_options_follow_foreign_options_with_values() {
        let input: DeriveInput = parse_quote! {
            #[graphql_entity(table = "records")]
            #[graphql(concrete(name = "Record", params(i32)), rename_fields = "snake_case")]
            #[serde(tag = "kind", rename_all = "camelCase")]
            struct Record {
                #[primary_key]
                id: String,
            }
        };
        let metadata = parse_entity_metadata(&input.attrs).unwrap();
        assert_eq!(
            metadata.graphql_rename_fields.as_deref(),
            Some("snake_case")
        );
        assert_eq!(metadata.serde_rename_all.as_deref(), Some("camelCase"));
    }

    #[test]
    fn field_options_follow_foreign_options_with_values() {
        let field: Field = Field::parse_named
            .parse2(quote::quote! {
                #[graphql(deprecation = "replaced", validator(min_length = 1), name = "renamed")]
                #[serde(skip_serializing_if = "String::is_empty", rename = "serdeName")]
                label: String
            })
            .unwrap();
        let meta = parse_field_metadata(&field).unwrap();
        assert_eq!(meta.graphql_name.as_deref(), Some("renamed"));
        assert_eq!(meta.serde_name.as_deref(), Some("serdeName"));

        let field: Field = Field::parse_named
            .parse2(quote::quote! {
                #[graphql(skip = false)]
                #[serde(rename(serialize = "a", deserialize = "b"))]
                label: String
            })
            .unwrap();
        let meta = parse_field_metadata(&field).unwrap();
        // `skip = false` keeps the legacy input exclusion (skip_false_compat_ui).
        assert!(meta.graphql_skip && meta.skip_input);
        assert!(!has_graphql_object_skip(&field));
        assert_eq!(meta.serde_name, None);
    }

    #[test]
    fn owned_field_attributes_reject_unknown_or_malformed_options() {
        for tokens in [
            quote::quote! { #[backup(encrypt)] label: String },
            quote::quote! { #[backup(include, redact)] label: String },
            quote::quote! { #[transform(write = "lower", encode = "x")] label: String },
            quote::quote! { #[filterable(type = "string", strict)] label: String },
            quote::quote! { #[graphql(name = "a" skip)] label: String },
            quote::quote! { #[serde(rename = 3)] label: String },
        ] {
            let field: Field = Field::parse_named.parse2(tokens.clone()).unwrap();
            assert!(parse_field_metadata(&field).is_err(), "{tokens}");
        }
    }
}
