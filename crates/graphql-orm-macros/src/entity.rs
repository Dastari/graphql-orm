use super::*;
use crate::backend::{
    BackendKind, backend_current_epoch_expr, backend_helper_import_tokens, backend_marker_tokens,
    backend_quote_identifier_path, backend_row_type_tokens, resolve_backend,
};
use crate::naming::{graphql_field_name, selected_field_case_rule};
use syn::spanned::Spanned;

#[derive(Default)]
pub(crate) struct EntityMetadata {
    pub(crate) backend: Option<String>,
    pub(crate) table_name: Option<String>,
    pub(crate) plural_name: Option<String>,
    pub(crate) default_sort: Option<String>,
    pub(crate) schema_policy: Option<String>,
    pub(crate) schema_only: bool,
    pub(crate) backup_enabled: Option<bool>,
    pub(crate) backup_export_order: Option<i32>,
    pub(crate) backup_restore_order: Option<i32>,
    pub(crate) read_policy: Option<String>,
    pub(crate) write_policy: Option<String>,
    /// Optional async hook path invoked after create/update/delete mutations.
    pub(crate) notify_handler: Option<String>,
    pub(crate) upsert: Option<Vec<String>>,
    pub(crate) unique_composite: Vec<Vec<String>>,
    pub(crate) indexes: Vec<(bool, Vec<String>)>,
    pub(crate) search: Option<SearchEntityMetadata>,
    pub(crate) serde_rename_all: Option<String>,
    pub(crate) graphql_rename_fields: Option<String>,
    pub(crate) rls: Option<RlsMetadata>,
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
}

pub(crate) fn parse_entity_metadata(attrs: &[syn::Attribute]) -> syn::Result<EntityMetadata> {
    let mut metadata = EntityMetadata::default();

    for attr in attrs {
        if attr.path().is_ident("graphql_entity") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("table") {
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
                } else if meta.path.is_ident("schema_only") {
                    let value = meta.value()?;
                    let lit: syn::LitBool = value.parse()?;
                    metadata.schema_only = lit.value;
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
                            "index must include at least one column",
                        ));
                    }
                    metadata.indexes.push((unique, cols));
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
                if meta.path.is_ident("search") {
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
                }
                Ok(())
            })?;
        } else if attr.path().is_ident("graphql") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename_fields") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    metadata.graphql_rename_fields = Some(lit.value());
                }
                Ok(())
            })?;
        } else if attr.path().is_ident("serde") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename_all") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    metadata.serde_rename_all = Some(lit.value());
                }
                Ok(())
            })?;
        }
    }

    Ok(metadata)
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
            } else if op.path.is_ident("owner") {
                operation.owner_column = Some(lit.value());
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

fn option_lit_tokens(value: Option<&str>, span: proc_macro2::Span) -> proc_macro2::TokenStream {
    value
        .map(|value| {
            let lit = syn::LitStr::new(value, span);
            quote! { Some(#lit) }
        })
        .unwrap_or_else(|| quote! { None })
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

pub(crate) fn has_graphql_complex(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("graphql") {
            return false;
        }

        let mut is_complex = false;
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("complex") {
                is_complex = true;
            }
            Ok(())
        });
        is_complex
    })
}

// ============================================================================
// Field Metadata Parsing
// ============================================================================

#[derive(Clone)]
pub(crate) struct FieldMetadata {
    pub(crate) graphql_name: Option<String>,
    pub(crate) serde_name: Option<String>,
    pub(crate) db_column: Option<String>,
    pub(crate) filterable: Option<String>,
    pub(crate) sortable: bool,
    pub(crate) unique: bool,
    pub(crate) is_primary_key: bool,
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
    pub(crate) skip_db: bool,
    /// Skip from generated public GraphQL Create/Update inputs only; field remains in DB,
    /// the Rust entity, and generated trusted Rust Create/Update input structs.
    pub(crate) skip_input: bool,
    pub(crate) is_private: bool,
    pub(crate) is_date_field: bool,
    pub(crate) is_boolean_field: bool,
    pub(crate) is_json_field: bool,
    pub(crate) spatial: Option<SpatialFieldMetadata>,
    pub(crate) search: Option<SearchFieldMetadata>,
    pub(crate) search_json: Vec<SearchJsonPathMetadata>,
    pub(crate) search_relation: Option<SearchRelationMetadata>,
    /// Async write transform: fn(&Context, String) -> Result<String>
    /// Applied before INSERT/UPDATE to transform the value (e.g., encryption)
    pub(crate) transform_write: Option<String>,
    /// Sync read transform: fn(T) -> T
    /// Applied after reading from the database row (e.g., decryption)
    pub(crate) transform_read: Option<String>,
    pub(crate) default: Option<String>,
    pub(crate) auto_generated: Option<bool>,
    pub(crate) backup_policy: Option<String>,
    /// If true, include in Create/Update inputs even if #[graphql(skip)] is set.
    /// Useful for fields that should be writable but never exposed in queries.
    pub(crate) input_only: bool,
    pub(crate) read: bool,
    pub(crate) write: bool,
    pub(crate) filter: bool,
    pub(crate) order: bool,
    pub(crate) subscribe: bool,
    pub(crate) read_policy: Option<String>,
    pub(crate) write_policy: Option<String>,
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
            filterable: None,
            sortable: false,
            unique: false,
            is_primary_key: false,
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
            skip_db: false,
            skip_input: false,
            is_private: false,
            is_date_field: false,
            is_boolean_field: false,
            is_json_field: false,
            spatial: None,
            search: None,
            search_json: Vec::new(),
            search_relation: None,
            transform_write: None,
            transform_read: None,
            default: None,
            auto_generated: None,
            backup_policy: None,
            input_only: false,
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
                    let _ = attr.parse_nested_meta(|nested| {
                        if nested.path.is_ident("name") {
                            let value = nested.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            meta.graphql_name = Some(lit.value());
                        } else if nested.path.is_ident("skip") {
                            meta.skip_input = true;
                        }
                        Ok(())
                    });
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
                        } else if nested.path.is_ident("default") {
                            let value = nested.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            meta.default = Some(lit.value());
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
                    let _ = attr.parse_nested_meta(|nested| {
                        if nested.path.is_ident("include") {
                            meta.backup_policy = Some("include".to_string());
                        } else if nested.path.is_ident("exclude") {
                            meta.backup_policy = Some("exclude".to_string());
                        } else if nested.path.is_ident("redact") {
                            meta.backup_policy = Some("redact".to_string());
                        }
                        Ok(())
                    });
                }
                "serde" => {
                    let _ = attr.parse_nested_meta(|nested| {
                        if nested.path.is_ident("rename") {
                            let value = nested.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            meta.serde_name = Some(lit.value());
                        }
                        Ok(())
                    });
                }
                "primary_key" => {
                    meta.is_primary_key = true;
                }
                "filterable" => {
                    if let Meta::List(_) = &attr.meta {
                        let _ = attr.parse_nested_meta(|nested| {
                            if nested.path.is_ident("type") {
                                let value = nested.value()?;
                                let lit: syn::LitStr = value.parse()?;
                                meta.filterable = Some(lit.value());
                            }
                            Ok(())
                        });
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
                    let _ = attr.parse_nested_meta(|nested| {
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
                        } else if nested.path.is_ident("multiple") {
                            meta.relation_multiple = true;
                        }
                        Ok(())
                    });
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
                    let _ = attr.parse_nested_meta(|nested| {
                        if nested.path.is_ident("write") {
                            let value = nested.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            meta.transform_write = Some(lit.value());
                        } else if nested.path.is_ident("read") {
                            let value = nested.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            meta.transform_read = Some(lit.value());
                        }
                        Ok(())
                    });
                }
                "input_only" => {
                    meta.input_only = true;
                }
                _ => {}
            }
        }
    }

    Ok(meta)
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
        .unwrap_or_else(|| "id".to_string());
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

/// Wrap a value expression with an async, context-aware write transform.
///
/// The transform function must have the signature:
///   `async fn(&::graphql_orm::async_graphql::Context<'_>, String) -> ::graphql_orm::async_graphql::Result<String>`
///
/// If `transform_write` is None, returns the original expression unchanged.
pub(crate) fn maybe_wrap_write_transform(
    expr: proc_macro2::TokenStream,
    transform_write: &Option<String>,
) -> proc_macro2::TokenStream {
    match transform_write {
        Some(path_str) => {
            let path: syn::Path = syn::parse_str(path_str)
                .unwrap_or_else(|_| syn::parse_str("unknown_transform").unwrap());
            quote! {
                {
                    let __raw_val: String = #expr;
                    #path(ctx, __raw_val).await?
                }
            }
        }
        None => expr,
    }
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

// ============================================================================
// GraphQL Entity Code Generation
// ============================================================================

pub(crate) fn generate_graphql_entity(
    input: &DeriveInput,
) -> syn::Result<proc_macro2::TokenStream> {
    generate_entity_impl(input, false)
}

pub(crate) fn generate_graphql_schema_entity(
    input: &DeriveInput,
) -> syn::Result<proc_macro2::TokenStream> {
    generate_entity_impl(input, true)
}

fn generate_entity_impl(
    input: &DeriveInput,
    schema_only_override: bool,
) -> syn::Result<proc_macro2::TokenStream> {
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
    let current_epoch_runtime = if backend == BackendKind::Postgres {
        quote! { "EXTRACT(EPOCH FROM NOW())::bigint" }
    } else if backend == BackendKind::Mssql {
        quote! { "DATEDIFF_BIG(second, '1970-01-01', SYSUTCDATETIME())" }
    } else {
        quote! { "unixepoch()" }
    };
    let current_date_runtime = if backend == BackendKind::Postgres {
        quote! { "CURRENT_DATE" }
    } else if backend == BackendKind::Mssql {
        quote! { "CAST(GETDATE() AS date)" }
    } else {
        quote! { "date('now')" }
    };
    let days_ago_runtime = if backend == BackendKind::Postgres {
        quote! { format!("(CURRENT_DATE - INTERVAL '{} days')::date", days) }
    } else if backend == BackendKind::Mssql {
        quote! { format!("DATEADD(day, -{}, CAST(GETDATE() AS date))", days) }
    } else {
        quote! { format!("date('now', '-{} days')", days) }
    };
    let days_ahead_runtime = if backend == BackendKind::Postgres {
        quote! { format!("(CURRENT_DATE + INTERVAL '{} days')::date", days) }
    } else if backend == BackendKind::Mssql {
        quote! { format!("DATEADD(day, {}, CAST(GETDATE() AS date))", days) }
    } else {
        quote! { format!("date('now', '+{} days')", days) }
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

    let schema_only = schema_only_override || entity_meta.schema_only;
    let entity_name_lit = struct_name.to_string();
    let backup_enabled = entity_meta.backup_enabled.unwrap_or(true);
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
    let legacy_graphql_complex = has_graphql_complex(&input.attrs);
    let raw_table_name = entity_meta.table_name.as_deref().unwrap_or("unknown");
    let table_name = backend_quote_identifier_path(backend, raw_table_name);
    let rls_impl = rls_impl_tokens(
        struct_name,
        &struct_name.to_string(),
        &table_name,
        entity_meta.rls.as_ref(),
    );
    let plural_name = entity_meta
        .plural_name
        .as_deref()
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{}s", struct_name));
    let raw_default_sort = entity_meta.default_sort.as_deref().unwrap_or("id");
    let default_sort =
        if backend == BackendKind::Mssql && !raw_default_sort.chars().any(char::is_whitespace) {
            backend_quote_identifier_path(backend, raw_default_sort)
        } else {
            raw_default_sort.to_string()
        };
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
    let mut index_defs = entity_meta
        .indexes
        .iter()
        .map(|(unique, cols)| {
            let name = format!("idx_{}_{}", raw_table_name, cols.join("_"));
            let col_lits = cols
                .iter()
                .map(|c| syn::LitStr::new(c, struct_name.span()))
                .collect::<Vec<_>>();

            if *unique {
                quote! {
                    ::graphql_orm::graphql::orm::IndexDef::new(#name, &[#(#col_lits),*]).unique()
                }
            } else {
                quote! {
                    ::graphql_orm::graphql::orm::IndexDef::new(#name, &[#(#col_lits),*])
                }
            }
        })
        .collect::<Vec<_>>();

    // Collect field info
    let mut column_names: Vec<String> = Vec::new();
    let mut column_defs: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut primary_key_cols: Vec<String> = Vec::new();
    let mut where_input_fields = Vec::new();
    let mut order_by_fields = Vec::new();
    let mut filter_to_sql = Vec::new();
    let mut filter_to_entity_match = Vec::new();
    let mut filter_is_empty_checks = Vec::new();
    let mut filter_contains_spatial_checks = Vec::new();
    let mut from_row_fields = Vec::new();
    let mut relation_metadata_defs = Vec::new();
    let mut search_field_defs = Vec::new();
    let mut search_json_path_defs = Vec::new();
    let mut search_relation_defs = Vec::new();
    let mut search_document_chunks = Vec::new();
    let mut sortable_columns: Vec<(syn::Ident, String)> = Vec::new();
    let mut object_field_methods = Vec::new();
    let parsed_fields = collect_parsed_fields(fields.iter())?;

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

        // Skip relation fields for column list
        if field_meta.is_relation || field_meta.skip_db {
            if field_meta.is_relation {
                validate_relation_delete_policy(struct_name, field, &field_meta, &parsed_fields)?;
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
                let source_columns = field_meta.relation_from_fields.clone().unwrap_or_else(|| {
                    vec![
                        field_meta
                            .relation_from
                            .clone()
                            .unwrap_or_else(|| "id".to_string()),
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
                let source_column = source_columns
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "id".to_string());
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
                    let search_weight =
                        search_weight_tokens(&search_relation.weight, field.span())?;
                    let search_fields = search_relation
                        .fields
                        .iter()
                        .map(|field| syn::LitStr::new(field, field_name.span()))
                        .collect::<Vec<_>>();
                    let max_items = search_relation.max_items;
                    let policy = option_lit_tokens(search_relation.policy.as_deref(), field.span());
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

                relation_metadata_defs.push(quote! {
                    ::graphql_orm::graphql::orm::RelationMetadata {
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
        let db_col = field_meta
            .db_column
            .clone()
            .unwrap_or_else(|| rust_name.clone());
        let db_col_sql = backend_quote_identifier_path(backend, &db_col);

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
                let index_name = format!("idx_{}_{}_spatial", raw_table_name, db_col);
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
            if !field_meta.read {
                return Err(syn::Error::new(
                    field.span(),
                    "private fields cannot be full-text searchable",
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
            if field_meta.is_private {
                return Err(syn::Error::new(
                    field.span(),
                    "private fields cannot use full-text search_json paths",
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
        let default_val = field_meta.default.clone().or_else(|| {
            if rust_name == "created_at" || rust_name == "updated_at" {
                Some(backend_current_epoch_expr(backend).to_string())
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
        let is_filterable = field_meta.filter && field_meta.filterable.is_some();

        column_defs.push(quote! {
            ::graphql_orm::graphql::orm::ColumnDef {
                name: #db_col,
                rust_name: #rust_name,
                sql_type: #sql_type,
                spatial: #spatial_tokens,
                search: #search_tokens,
                logical_type: #logical_type,
                nullable: #is_nullable,
                is_primary_key: #is_pk,
                is_unique: #is_unique,
                is_generated: #is_generated,
                is_filterable: #is_filterable,
                backup_policy: #backup_policy,
                default: #default_expr,
                references: None,
            }
        });

        if field_meta.is_primary_key {
            primary_key_cols.push(db_col_sql.clone());
        }

        // Generate WhereInput field for filterable fields
        if field_meta.filter {
            if let Some(ref filter_type) = field_meta.filterable {
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
            }
        }

        // Generate OrderByInput field for sortable fields
        if field_meta.sortable && field_meta.order {
            let order_field_name = rust_ident_from_graphql_name(&graphql_name, field_name.span());
            sortable_columns.push((order_field_name.clone(), db_col_sql.clone()));
            order_by_fields.push(quote! {
                #[graphql(name = #graphql_name)]
                pub #order_field_name: Option<::graphql_orm::graphql::orm::OrderDirection>,
            });
        }

        if field_meta.read && !field_meta.input_only {
            let getter_name = field_name;
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
            let policy_check = if let Some(policy_key) = &field_meta.read_policy {
                quote! {
                    db.ensure_readable_field(
                        ctx,
                        #entity_name_lit,
                        #graphql_name,
                        Some(#policy_key),
                        Some(self as &(dyn ::std::any::Any + Send + Sync)),
                    ).await?;
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
        }

        // Generate FromSqlRow field assignment
        let row_assignment =
            generate_row_field_assignment(backend, field_name, field_type, &db_col, &field_meta)?;
        from_row_fields.push(row_assignment);
    }

    let default_primary_key = if backend == BackendKind::Mssql {
        backend_quote_identifier_path(backend, "id")
    } else {
        "id".to_string()
    };
    if primary_key_cols.is_empty() {
        primary_key_cols.push(default_primary_key);
    }
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
    let search_index_name = format!("idx_gom_search_{}_vector", raw_table_name.replace('.', "_"));
    let search_pk_field = parsed_fields
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
        })
        .and_then(|parsed| parsed.field.ident.clone())
        .unwrap_or_else(|| syn::Ident::new("id", struct_name.span()));
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
        quote! {
            impl ::graphql_orm::graphql::orm::SearchableEntity for #struct_name {
                fn search_key(&self) -> String {
                    self.#search_pk_field.to_string()
                }

                fn search_key_json(&self) -> ::graphql_orm::serde_json::Value {
                    ::graphql_orm::serde_json::to_value(&self.#search_pk_field)
                        .unwrap_or_else(|_| ::graphql_orm::serde_json::Value::String(self.#search_pk_field.to_string()))
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

    let object_impl = if schema_only || legacy_graphql_complex {
        quote! {}
    } else {
        quote! {
            #[::graphql_orm::async_graphql::Object]
            impl #struct_name {
                #(#object_field_methods)*
            }
        }
    };

    if schema_only {
        return Ok(quote! {
            #rls_impl
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
            }

            impl ::graphql_orm::graphql::orm::EntityRelations for #struct_name {
                fn relation_metadata() -> &'static [::graphql_orm::graphql::orm::RelationMetadata] {
                    static RELATIONS: &[::graphql_orm::graphql::orm::RelationMetadata] = &[
                        #(#relation_metadata_defs),*
                    ];
                    RELATIONS
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
                        ::graphql_orm::graphql::orm::EntityMetadata::from_schema::<Self>(
                            #struct_name_str,
                            #backup_enabled,
                            #backup_export_order,
                            #backup_restore_order,
                            #read_policy,
                            #write_policy,
                        )
                    })
                }
            }
        });
    }

    Ok(quote! {
        #rls_impl
        #search_schema_impl
        #searchable_entity_impl

        // WhereInput for filtering
        #[derive(::graphql_orm::async_graphql::InputObject, Default, Clone, Debug)]
        #[graphql(name = #where_input_name_str, rename_fields = #field_case_rule)]
        pub struct #where_input_name {
            #(#where_input_fields)*

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
        }

        #object_impl

        impl ::graphql_orm::graphql::orm::DatabaseOrderBy for #order_by_name {
            fn to_sql_order(&self) -> Option<String> {
                let mut parts = Vec::new();
                #(#order_by_match_arms)*
                if parts.is_empty() {
                    None
                } else {
                    Some(parts.join(", "))
                }
            }
        }

        impl ::graphql_orm::graphql::orm::DatabaseFilter for #where_input_name {
            fn to_sql_conditions(&self) -> (Vec<String>, Vec<::graphql_orm::graphql::orm::SqlValue>) {
                let mut conditions = Vec::new();
                let mut values = Vec::new();

                #(#filter_to_sql)*

                // Handle And
                if let Some(ref and_filters) = self.and {
                    for filter in and_filters {
                        let (sub_conds, sub_vals) = filter.to_sql_conditions();
                        conditions.extend(sub_conds);
                        values.extend(sub_vals);
                    }
                }

                // Handle Or
                if let Some(ref or_filters) = self.or {
                    let mut or_parts = Vec::new();
                    for filter in or_filters {
                        let (sub_conds, sub_vals) = filter.to_sql_conditions();
                        if !sub_conds.is_empty() {
                            or_parts.push(format!("({})", sub_conds.join(" AND ")));
                            values.extend(sub_vals);
                        }
                    }
                    if !or_parts.is_empty() {
                        conditions.push(format!("({})", or_parts.join(" OR ")));
                    }
                }

                // Handle Not
                if let Some(ref not_filter) = self.not {
                    let (sub_conds, sub_vals) = not_filter.to_sql_conditions();
                    if !sub_conds.is_empty() {
                        conditions.push(format!("NOT ({})", sub_conds.join(" AND ")));
                        values.extend(sub_vals);
                    }
                }

                (conditions, values)
            }

            fn is_empty(&self) -> bool {
                self.__gom_is_empty()
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
            ) -> Result<bool, ::graphql_orm::sqlx::Error> {
                let entity = entity.downcast_ref::<#struct_name>().ok_or_else(|| {
                    ::graphql_orm::sqlx::Error::Decode(Box::new(::std::io::Error::new(
                        ::std::io::ErrorKind::InvalidData,
                        concat!("in-memory filter entity type mismatch for ", stringify!(#struct_name)),
                    )))
                })?;
                self.__gom_matches_entity(entity)
            }
        }

        impl #where_input_name {
            fn __gom_to_sql_prefilter_conditions(&self) -> (Vec<String>, Vec<::graphql_orm::graphql::orm::SqlValue>) {
                let mut conditions = Vec::new();
                let mut values = Vec::new();

                #(#filter_to_sql)*

                if let Some(ref and_filters) = self.and {
                    for filter in and_filters {
                        let (sub_conds, sub_vals) = filter.__gom_to_sql_prefilter_conditions();
                        conditions.extend(sub_conds);
                        values.extend(sub_vals);
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
                    if or_filters.iter().any(|filter| !filter.__gom_is_empty()) {
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

            fn __gom_matches_entity(&self, entity: &#struct_name) -> Result<bool, ::graphql_orm::sqlx::Error> {
                #(#filter_to_entity_match)*

                if let Some(ref and_filters) = self.and {
                    for filter in and_filters {
                        if !filter.__gom_matches_entity(entity)? {
                            return Ok(false);
                        }
                    }
                }

                if let Some(ref or_filters) = self.or {
                    let mut matched = false;
                    for filter in or_filters {
                        if filter.__gom_matches_entity(entity)? {
                            matched = true;
                            break;
                        }
                    }
                    if !matched {
                        return Ok(false);
                    }
                }

                if let Some(ref not_filter) = self.not {
                    if not_filter.__gom_matches_entity(entity)? {
                        return Ok(false);
                    }
                }

                Ok(true)
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

            pub(crate) fn __gom_current_epoch_expr() -> &'static str {
                #current_epoch_runtime
            }

            pub(crate) fn __gom_current_date_expr() -> &'static str {
                #current_date_runtime
            }

            pub(crate) fn __gom_days_ago_expr(days: i64) -> String {
                #days_ago_runtime
            }

            pub(crate) fn __gom_days_ahead_expr(days: i64) -> String {
                #days_ahead_runtime
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
        }

        impl ::graphql_orm::graphql::orm::EntityRelations for #struct_name {
            fn relation_metadata() -> &'static [::graphql_orm::graphql::orm::RelationMetadata] {
                static RELATIONS: &[::graphql_orm::graphql::orm::RelationMetadata] = &[
                    #(#relation_metadata_defs),*
                ];
                RELATIONS
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
                    ::graphql_orm::graphql::orm::EntityMetadata::from_schema::<Self>(
                        #struct_name_str,
                        #backup_enabled,
                        #backup_export_order,
                        #backup_restore_order,
                        #read_policy,
                        #write_policy,
                    )
                })
            }
        }

        impl ::graphql_orm::graphql::orm::FromSqlRow<#backend_marker> for #struct_name {
            fn from_row(row: &#row_type) -> Result<Self, ::graphql_orm::sqlx::Error> {
                #helper_import

                Ok(Self {
                    #(#from_row_fields)*
                })
            }
        }
    })
}

// ============================================================================
// Filter Field Generation
// ============================================================================

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
                            return Ok(false);
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
        "string" => {
            let input = quote! {
                #[graphql(name = #graphql_name)]
                pub #filter_field_name: Option<::graphql_orm::graphql::filters::StringFilter>,
            };
            let sql = quote! {
                if let Some(ref f) = self.#filter_field_name {
                    if let Some(ref v) = f.eq {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(format!("{} = {}", #db_col, placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::String(v.clone()));
                    }
                    if let Some(ref v) = f.ne {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(format!("{} != {}", #db_col, placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::String(v.clone()));
                    }
                    if let Some(ref v) = f.contains {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(#struct_name::__gom_ci_like(#db_col, &placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::String(
                            ::graphql_orm::graphql::orm::contains_like_pattern(v)
                        ));
                    }
                    if let Some(ref v) = f.starts_with {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(#struct_name::__gom_ci_like(#db_col, &placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::String(
                            ::graphql_orm::graphql::orm::starts_with_like_pattern(v)
                        ));
                    }
                    if let Some(ref v) = f.ends_with {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(#struct_name::__gom_ci_like(#db_col, &placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::String(
                            ::graphql_orm::graphql::orm::ends_with_like_pattern(v)
                        ));
                    }
                    if let Some(ref list) = f.in_list {
                        if !list.is_empty() {
                            let base_index = values.len() + 1;
                            let placeholders: Vec<String> = (0..list.len())
                                .map(|offset| #struct_name::__gom_placeholder(base_index + offset))
                                .collect();
                            conditions.push(format!("{} IN ({})", #db_col, placeholders.join(", ")));
                            for v in list {
                                values.push(::graphql_orm::graphql::orm::SqlValue::String(v.clone()));
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
                                values.push(::graphql_orm::graphql::orm::SqlValue::String(v.clone()));
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
                    // Similar matching uses LIKE for candidate filtering.
                    // Actual scoring happens in Rust post-processing
                    if let Some(ref sim) = f.similar {
                        // Use a broad LIKE pattern to get candidates
                        // Generated fallback paths use deterministic substring scoring after fetch.
                        let pattern = ::graphql_orm::graphql::orm::generate_candidate_pattern(&sim.value);
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(#struct_name::__gom_ci_like(#db_col, &placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::String(pattern));
                    }
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
                            return Ok(false);
                        }
                    }
                }
            } else {
                quote! {}
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
                    if let Some(v) = f.eq {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(format!("{} = {}", #db_col, placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::Int(v as i64));
                    }
                    if let Some(v) = f.ne {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(format!("{} != {}", #db_col, placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::Int(v as i64));
                    }
                    if let Some(v) = f.lt {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(format!("{} < {}", #db_col, placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::Int(v as i64));
                    }
                    if let Some(v) = f.lte {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(format!("{} <= {}", #db_col, placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::Int(v as i64));
                    }
                    if let Some(v) = f.gt {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(format!("{} > {}", #db_col, placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::Int(v as i64));
                    }
                    if let Some(v) = f.gte {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(format!("{} >= {}", #db_col, placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::Int(v as i64));
                    }
                    if let Some(ref list) = f.in_list {
                        if !list.is_empty() {
                            let base_index = values.len() + 1;
                            let placeholders: Vec<String> = (0..list.len())
                                .map(|offset| #struct_name::__gom_placeholder(base_index + offset))
                                .collect();
                            conditions.push(format!("{} IN ({})", #db_col, placeholders.join(", ")));
                            for v in list {
                                values.push(::graphql_orm::graphql::orm::SqlValue::Int(*v as i64));
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
                                values.push(::graphql_orm::graphql::orm::SqlValue::Int(*v as i64));
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
                quote! { entity.#field_name.map(|value| value as i64) }
            } else {
                quote! { Some(entity.#field_name as i64) }
            };
            let entity_match = if backend == BackendKind::Sqlite {
                quote! {
                    if let Some(ref f) = self.#filter_field_name {
                        if !::graphql_orm::graphql::orm::spatial::int_filter_matches(#value_expr, f) {
                            return Ok(false);
                        }
                    }
                }
            } else {
                quote! {}
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
                        if !list.is_empty() {
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
                            return Ok(false);
                        }
                    }
                }
            } else {
                quote! {}
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
                    if let Some(v) = f.eq {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(format!("{} = {}", #db_col, placeholder));
                        values.push(#struct_name::__gom_bool_sql_value(v));
                    }
                    if let Some(v) = f.ne {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(format!("{} != {}", #db_col, placeholder));
                        values.push(#struct_name::__gom_bool_sql_value(v));
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
                        if !::graphql_orm::graphql::orm::spatial::bool_filter_matches(#value_expr, f) {
                            return Ok(false);
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
                    if let Some(ref v) = f.eq {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(format!("{} = {}", #db_col, placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::String(v.clone()));
                    }
                    if let Some(ref v) = f.ne {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(format!("{} != {}", #db_col, placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::String(v.clone()));
                    }
                    if let Some(ref v) = f.lt {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(format!("{} < {}", #db_col, placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::String(v.clone()));
                    }
                    if let Some(ref v) = f.lte {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(format!("{} <= {}", #db_col, placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::String(v.clone()));
                    }
                    if let Some(ref v) = f.gt {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(format!("{} > {}", #db_col, placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::String(v.clone()));
                    }
                    if let Some(ref v) = f.gte {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(format!("{} >= {}", #db_col, placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::String(v.clone()));
                    }
                    if let Some(ref range) = f.between {
                        if let (Some(start), Some(end)) = (&range.start, &range.end) {
                            let start_placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                            let end_placeholder = #struct_name::__gom_placeholder(values.len() + 2);
                            conditions.push(format!("{} BETWEEN {} AND {}", #db_col, start_placeholder, end_placeholder));
                            values.push(::graphql_orm::graphql::orm::SqlValue::String(start.clone()));
                            values.push(::graphql_orm::graphql::orm::SqlValue::String(end.clone()));
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
                    // Date arithmetic operators
                    if f.in_past == Some(true) {
                        conditions.push(format!("{} < {}", #db_col, #struct_name::__gom_current_date_expr()));
                    }
                    if f.in_future == Some(true) {
                        conditions.push(format!("{} > {}", #db_col, #struct_name::__gom_current_date_expr()));
                    }
                    if f.is_today == Some(true) {
                        conditions.push(format!("{} = {}", #db_col, #struct_name::__gom_current_date_expr()));
                    }
                    if let Some(days) = f.recent_days {
                        // Within the last N days (inclusive of today)
                        conditions.push(format!(
                            "{} >= {} AND {} <= {}",
                            #db_col,
                            #struct_name::__gom_days_ago_expr(days.into()),
                            #db_col,
                            #struct_name::__gom_current_date_expr()
                        ));
                    }
                    if let Some(days) = f.within_days {
                        // Within the next N days (inclusive of today)
                        conditions.push(format!(
                            "{} >= {} AND {} <= {}",
                            #db_col,
                            #struct_name::__gom_current_date_expr(),
                            #db_col,
                            #struct_name::__gom_days_ahead_expr(days.into())
                        ));
                    }
                    if let Some(ref rel) = f.gte_relative {
                        let expr = rel.to_sql_expr(#backend_expr);
                        conditions.push(format!("{} >= {}", #db_col, expr));
                    }
                    if let Some(ref rel) = f.lte_relative {
                        let expr = rel.to_sql_expr(#backend_expr);
                        conditions.push(format!("{} <= {}", #db_col, expr));
                    }
                }
            };
            let entity_match = if backend == BackendKind::Sqlite {
                if is_option_type(field_type) {
                    quote! {
                        if let Some(ref f) = self.#filter_field_name {
                            let __gom_date_value = entity.#field_name.as_ref().map(|value| value.to_string());
                            if !::graphql_orm::graphql::orm::spatial::date_filter_matches(__gom_date_value.as_deref(), f) {
                                return Ok(false);
                            }
                        }
                    }
                } else {
                    quote! {
                        if let Some(ref f) = self.#filter_field_name {
                            let __gom_date_value = entity.#field_name.to_string();
                            if !::graphql_orm::graphql::orm::spatial::date_filter_matches(Some(__gom_date_value.as_str()), f) {
                                return Ok(false);
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
