use super::*;
use crate::backend::{
    backend_current_epoch_expr, backend_helper_import_tokens, backend_row_type_tokens,
};
use syn::spanned::Spanned;

#[derive(Default)]
pub(crate) struct EntityMetadata {
    pub(crate) table_name: Option<String>,
    pub(crate) plural_name: Option<String>,
    pub(crate) default_sort: Option<String>,
    pub(crate) schema_only: bool,
    pub(crate) read_policy: Option<String>,
    pub(crate) write_policy: Option<String>,
    /// Optional async hook path invoked after create/update/delete mutations.
    pub(crate) notify_handler: Option<String>,
    pub(crate) unique_composite: Vec<Vec<String>>,
    pub(crate) indexes: Vec<(bool, Vec<String>)>,
    pub(crate) serde_rename_all: Option<String>,
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
                } else if meta.path.is_ident("plural") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    metadata.plural_name = Some(lit.value());
                } else if meta.path.is_ident("default_sort") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    metadata.default_sort = Some(lit.value());
                } else if meta.path.is_ident("schema_only") {
                    let value = meta.value()?;
                    let lit: syn::LitBool = value.parse()?;
                    metadata.schema_only = lit.value;
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
    pub(crate) relation_multiple: bool,
    pub(crate) relation_on_delete: Option<String>,
    pub(crate) skip_db: bool,
    /// Skip from Create/Update inputs only (e.g. password_hash); field remains in DB and struct
    pub(crate) skip_input: bool,
    pub(crate) is_date_field: bool,
    pub(crate) is_boolean_field: bool,
    pub(crate) is_json_field: bool,
    /// Async write transform: fn(&Context, String) -> Result<String>
    /// Applied before INSERT/UPDATE to transform the value (e.g., encryption)
    pub(crate) transform_write: Option<String>,
    /// Sync read transform: fn(T) -> T
    /// Applied after reading from the database row (e.g., decryption)
    pub(crate) transform_read: Option<String>,
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
            relation_multiple: false,
            relation_on_delete: None,
            skip_db: false,
            skip_input: false,
            is_date_field: false,
            is_boolean_field: false,
            is_json_field: false,
            transform_write: None,
            transform_read: None,
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
                    let _ = attr.parse_nested_meta(|nested| {
                        if nested.path.is_ident("private") {
                            meta.read = false;
                            meta.filter = false;
                            meta.order = false;
                            meta.subscribe = false;
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
                            let lit: syn::LitStr = value.parse()?;
                            meta.relation_from = Some(lit.value());
                        } else if nested.path.is_ident("to") {
                            let value = nested.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            meta.relation_to = Some(lit.value());
                        } else if nested.path.is_ident("on_delete") {
                            let value = nested.value()?;
                            let lit: syn::LitStr = value.parse()?;
                            meta.relation_on_delete = Some(lit.value());
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

fn apply_rename_rule(name: &str, rule: &str) -> String {
    match rule {
        "lowercase" => name.to_case(Case::Lower),
        "UPPERCASE" => name.to_case(Case::Upper),
        "camelCase" => name.to_case(Case::Camel),
        "PascalCase" => name.to_case(Case::Pascal),
        "snake_case" => name.to_case(Case::Snake),
        "SCREAMING_SNAKE_CASE" => name.to_case(Case::UpperSnake),
        "kebab-case" => name.to_case(Case::Kebab),
        "SCREAMING-KEBAB-CASE" => name.to_case(Case::UpperKebab),
        _ => name.to_string(),
    }
}

pub(crate) fn graphql_field_name(
    meta: &FieldMetadata,
    rust_name: &str,
    rename_all: Option<&str>,
) -> String {
    if let Some(graphql_name) = &meta.graphql_name {
        graphql_name.clone()
    } else if let Some(serde_name) = &meta.serde_name {
        serde_name.clone()
    } else if let Some(rule) = rename_all {
        apply_rename_rule(rust_name, rule)
    } else {
        rust_name.to_case(Case::Camel)
    }
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
    let row_type = backend_row_type_tokens();
    let helper_import = backend_helper_import_tokens();
    let placeholder_body = if cfg!(feature = "postgres") {
        quote! { format!("${}", index) }
    } else {
        quote! { "?".to_string() }
    };
    let rebind_loop_body = if cfg!(feature = "postgres") {
        quote! {
            if chars[i] == '?' {
                rebound.push_str(&Self::__gom_placeholder(next_index));
                next_index += 1;
                i += 1;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
            } else if chars[i] == '$' {
                rebound.push_str(&Self::__gom_placeholder(next_index));
                next_index += 1;
                i += 1;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
            } else {
                rebound.push(chars[i]);
                i += 1;
            }
        }
    } else {
        quote! {
            if chars[i] == '?' {
                rebound.push_str(&Self::__gom_placeholder(next_index));
                next_index += 1;
                i += 1;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
            } else {
                rebound.push(chars[i]);
                i += 1;
            }
        }
    };
    let ci_like_body = if cfg!(feature = "postgres") {
        quote! { format!("{} ILIKE {}", column, placeholder) }
    } else {
        quote! { format!("LOWER({}) LIKE LOWER({})", column, placeholder) }
    };
    let bool_sql_value_body = if cfg!(feature = "postgres") {
        quote! { ::graphql_orm::graphql::orm::SqlValue::Bool(value) }
    } else {
        quote! { ::graphql_orm::graphql::orm::SqlValue::Int(if value { 1 } else { 0 }) }
    };
    let current_epoch_runtime = if cfg!(feature = "postgres") {
        quote! { "EXTRACT(EPOCH FROM NOW())::bigint" }
    } else {
        quote! { "unixepoch()" }
    };
    let current_date_runtime = if cfg!(feature = "postgres") {
        quote! { "CURRENT_DATE" }
    } else {
        quote! { "date('now')" }
    };
    let days_ago_runtime = if cfg!(feature = "postgres") {
        quote! { format!("(CURRENT_DATE - INTERVAL '{} days')::date", days) }
    } else {
        quote! { format!("date('now', '-{} days')", days) }
    };
    let days_ahead_runtime = if cfg!(feature = "postgres") {
        quote! { format!("(CURRENT_DATE + INTERVAL '{} days')::date", days) }
    } else {
        quote! { format!("date('now', '+{} days')", days) }
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

    let entity_meta = parse_entity_metadata(&input.attrs)?;
    let schema_only = schema_only_override || entity_meta.schema_only;
    let entity_name_lit = struct_name.to_string();
    let rename_all_rule = entity_meta.serde_rename_all.as_deref();
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
    let table_name = entity_meta.table_name.as_deref().unwrap_or("unknown");
    let plural_name = entity_meta
        .plural_name
        .as_deref()
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{}s", struct_name));
    let default_sort = entity_meta.default_sort.as_deref().unwrap_or("id");
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
    let index_defs = entity_meta
        .indexes
        .iter()
        .map(|(unique, cols)| {
            let name = format!("idx_{}_{}", table_name, cols.join("_"));
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
    let mut primary_key_col: Option<String> = None;
    let mut where_input_fields = Vec::new();
    let mut order_by_fields = Vec::new();
    let mut filter_to_sql = Vec::new();
    let mut from_row_fields = Vec::new();
    let mut relation_metadata_defs = Vec::new();
    let mut sortable_columns: Vec<String> = Vec::new();
    let mut object_field_methods = Vec::new();
    let parsed_fields = collect_parsed_fields(fields.iter())?;

    for parsed_field in &parsed_fields {
        let field = &parsed_field.field;
        let field_name = field.ident.as_ref().unwrap();
        let field_type = &field.ty;
        let field_meta = parsed_field.meta.clone();

        // Skip relation fields for column list
        if field_meta.is_relation || field_meta.skip_db {
            if field_meta.is_relation {
                validate_relation_delete_policy(struct_name, field, &field_meta, &parsed_fields)?;
                let rust_name = field_name.to_string();
                let graphql_name = graphql_field_name(&field_meta, &rust_name, rename_all_rule);
                let target_type = field_meta
                    .relation_target
                    .clone()
                    .unwrap_or_else(|| "Unknown".to_string());
                let source_column = field_meta
                    .relation_from
                    .clone()
                    .unwrap_or_else(|| "id".to_string());
                let target_column = field_meta
                    .relation_to
                    .clone()
                    .unwrap_or_else(|| "unknown_id".to_string());
                let is_multiple = field_meta.relation_multiple;
                let on_delete = relation_delete_policy_tokens(
                    field_meta.relation_on_delete.as_deref(),
                    field.span(),
                )?;

                relation_metadata_defs.push(quote! {
                    ::graphql_orm::graphql::orm::RelationMetadata {
                        field_name: #graphql_name,
                        target_type: #target_type,
                        source_column: #source_column,
                        target_column: #target_column,
                        is_multiple: #is_multiple,
                        on_delete: #on_delete,
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
        let graphql_name = graphql_field_name(&field_meta, &rust_name, rename_all_rule);
        let db_col = field_meta
            .db_column
            .clone()
            .unwrap_or_else(|| rust_name.clone());

        column_names.push(db_col.clone());

        // Determine SQL type and nullability
        let is_nullable = is_option_type(field_type);
        let is_pk = field_meta.is_primary_key;
        let is_unique = field_meta.unique;
        let sql_type = rust_type_to_sql_type(field_type, &field_meta);
        let default_val = if rust_name == "created_at" || rust_name == "updated_at" {
            Some(backend_current_epoch_expr())
        } else {
            None
        };

        // Build column definition
        let default_expr = match default_val {
            Some(d) => quote! { Some(#d) },
            None => quote! { None },
        };

        column_defs.push(quote! {
            ::graphql_orm::graphql::orm::ColumnDef {
                name: #db_col,
                sql_type: #sql_type,
                nullable: #is_nullable,
                is_primary_key: #is_pk,
                is_unique: #is_unique,
                default: #default_expr,
                references: None,
            }
        });

        if field_meta.is_primary_key {
            primary_key_col = Some(db_col.clone());
        }

        // Generate WhereInput field for filterable fields
        if field_meta.filter {
            if let Some(ref filter_type) = field_meta.filterable {
                let (input_field, sql_gen) = generate_filter_field(
                    struct_name,
                    field_name,
                    &graphql_name,
                    &db_col,
                    filter_type,
                )?;
                where_input_fields.push(input_field);
                filter_to_sql.push(sql_gen);
            }
        }

        // Generate OrderByInput field for sortable fields
        if field_meta.sortable && field_meta.order {
            sortable_columns.push(db_col.clone());
            let order_field_name =
                syn::Ident::new(&to_snake_case(&graphql_name), field_name.span());
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
            let return_expr = if is_option_type(field_type) || is_byte_vec_type(field_type) {
                quote! { Ok(self.#field_name.clone()) }
            } else {
                quote! { Ok(self.#field_name.clone()) }
            };
            object_field_methods.push(quote! {
                #[graphql(name = #graphql_name)]
                async fn #getter_name(
                    &self,
                    ctx: &::graphql_orm::async_graphql::Context<'_>,
                ) -> ::graphql_orm::async_graphql::Result<#field_type> {
                    let db = ctx.data_unchecked::<::graphql_orm::db::Database>();
                    #subscribe_check
                    #policy_check
                    #return_expr
                }
            });
        }

        // Generate FromSqlRow field assignment
        let row_assignment =
            generate_row_field_assignment(field_name, field_type, &db_col, &field_meta)?;
        from_row_fields.push(row_assignment);
    }

    let primary_key = primary_key_col.as_deref().unwrap_or("id");
    let columns_array: Vec<&str> = column_names.iter().map(|s| s.as_str()).collect();

    // Generate type names (as strings for #[graphql(name = "...")] and as idents for struct names)
    let where_input_name_str = format!("{}WhereInput", struct_name);
    let order_by_name_str = format!("{}OrderByInput", struct_name);
    let where_input_name = syn::Ident::new(&where_input_name_str, struct_name.span());
    let order_by_name = syn::Ident::new(&order_by_name_str, struct_name.span());
    let struct_name_str = struct_name.to_string();

    // Generate order_by to_sql_order implementation
    let order_by_match_arms: Vec<_> = sortable_columns
        .iter()
        .map(|col| {
            let field_name = syn::Ident::new(&to_snake_case(col), struct_name.span());
            quote! {
                if let Some(dir) = &self.#field_name {
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
            impl ::graphql_orm::graphql::orm::DatabaseEntity for #struct_name {
                const TABLE_NAME: &'static str = #table_name;
                const PLURAL_NAME: &'static str = #plural_name;
                const PRIMARY_KEY: &'static str = #primary_key;
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
                            #read_policy,
                            #write_policy,
                        )
                    })
                }
            }
        });
    }

    Ok(quote! {
        // WhereInput for filtering
        #[derive(::graphql_orm::async_graphql::InputObject, Default, Clone, Debug)]
        #[graphql(name = #where_input_name_str)]
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
        #[graphql(name = #order_by_name_str)]
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
                // Check if all filter fields are None/empty
                let (conds, _) = self.to_sql_conditions();
                conds.is_empty()
            }
        }

        impl ::graphql_orm::graphql::orm::DatabaseEntity for #struct_name {
            const TABLE_NAME: &'static str = #table_name;
            const PLURAL_NAME: &'static str = #plural_name;
            const PRIMARY_KEY: &'static str = #primary_key;
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
                let mut rebound = String::with_capacity(sql.len() + 16);
                let chars: Vec<char> = sql.chars().collect();
                let mut i = 0usize;
                let mut next_index = start_index;

                while i < chars.len() {
                    #rebind_loop_body
                }

                rebound
            }

            pub(crate) fn __gom_ci_like(column: &str, placeholder: &str) -> String {
                #ci_like_body
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
                        #read_policy,
                        #write_policy,
                    )
                })
            }
        }

        impl ::graphql_orm::graphql::orm::FromSqlRow for #struct_name {
            fn from_row(row: &#row_type) -> Result<Self, ::graphql_orm::sqlx::Error> {
                use ::graphql_orm::sqlx::Row;
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
    struct_name: &syn::Ident,
    field_name: &syn::Ident,
    graphql_name: &str,
    db_col: &str,
    filter_type: &str,
) -> syn::Result<(proc_macro2::TokenStream, proc_macro2::TokenStream)> {
    let filter_field_name = syn::Ident::new(&to_snake_case(graphql_name), field_name.span());

    match filter_type {
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
                        values.push(::graphql_orm::graphql::orm::SqlValue::String(format!("%{}%", v)));
                    }
                    if let Some(ref v) = f.starts_with {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(#struct_name::__gom_ci_like(#db_col, &placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::String(format!("{}%", v)));
                    }
                    if let Some(ref v) = f.ends_with {
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(#struct_name::__gom_ci_like(#db_col, &placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::String(format!("%{}", v)));
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
                    // Similar/fuzzy matching - use LIKE for candidate filtering
                    // Actual scoring happens in Rust post-processing
                    if let Some(ref sim) = f.similar {
                        // Use a broad LIKE pattern to get candidates
                        // Fuzzy scoring with strsim happens after fetch
                        let pattern = ::graphql_orm::graphql::orm::generate_candidate_pattern(&sim.value);
                        let placeholder = #struct_name::__gom_placeholder(values.len() + 1);
                        conditions.push(#struct_name::__gom_ci_like(#db_col, &placeholder));
                        values.push(::graphql_orm::graphql::orm::SqlValue::String(pattern));
                    }
                }
            };
            Ok((input, sql))
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
            Ok((input, sql))
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
            Ok((input, sql))
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
            Ok((input, sql))
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
                        let expr = rel.to_sql_expr();
                        conditions.push(format!("{} >= {}", #db_col, expr));
                    }
                    if let Some(ref rel) = f.lte_relative {
                        let expr = rel.to_sql_expr();
                        conditions.push(format!("{} <= {}", #db_col, expr));
                    }
                }
            };
            Ok((input, sql))
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
    field_name: &syn::Ident,
    field_type: &syn::Type,
    db_col: &str,
    meta: &FieldMetadata,
) -> syn::Result<proc_macro2::TokenStream> {
    let bool_expr = if cfg!(feature = "postgres") {
        quote! { row.try_get::<bool, _>(#db_col)? }
    } else {
        quote! {{
            let i: i32 = row.try_get(#db_col)?;
            int_to_bool(i)
        }}
    };
    let uuid_expr = if cfg!(feature = "postgres") {
        quote! { row.try_get(#db_col)? }
    } else {
        quote! {{
            let s: String = row.try_get(#db_col)?;
            str_to_uuid(&s).map_err(|e| ::graphql_orm::sqlx::Error::Decode(e.into()))?
        }}
    };
    let datetime_expr = if cfg!(feature = "postgres") {
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

    if meta.is_json_field && !is_option_type(field_type) {
        let json_expr = if cfg!(feature = "postgres") {
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
                                field_name, inner_type, db_col, meta,
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
    field_name: &syn::Ident,
    inner_type: &syn::Type,
    db_col: &str,
    meta: &FieldMetadata,
) -> syn::Result<proc_macro2::TokenStream> {
    let optional_bool_expr = if cfg!(feature = "postgres") {
        quote! { row.try_get(#db_col)? }
    } else {
        quote! {{
            let i: Option<i32> = row.try_get(#db_col)?;
            i.map(int_to_bool)
        }}
    };
    let optional_uuid_expr = if cfg!(feature = "postgres") {
        quote! { row.try_get(#db_col)? }
    } else {
        quote! {{
            let s: Option<String> = row.try_get(#db_col)?;
            s.map(|s| str_to_uuid(&s))
                .transpose()
                .map_err(|e| ::graphql_orm::sqlx::Error::Decode(e.into()))?
        }}
    };
    let optional_datetime_expr = if cfg!(feature = "postgres") {
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
                let json_expr = if cfg!(feature = "postgres") {
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

/// Convert Rust type to the configured backend SQL type string
pub(crate) fn rust_type_to_sql_type(ty: &syn::Type, meta: &FieldMetadata) -> &'static str {
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
    if meta.is_boolean_field {
        return if cfg!(feature = "postgres") {
            "BOOLEAN"
        } else {
            "INTEGER"
        };
    }
    if meta.is_json_field {
        return if cfg!(feature = "postgres") {
            "JSONB"
        } else {
            "TEXT"
        };
    }
    if meta.is_date_field {
        return if cfg!(feature = "postgres") {
            "TIMESTAMPTZ"
        } else {
            "TEXT"
        };
    }

    // Infer from Rust type first (so f64 becomes REAL not INTEGER for "number" filter)
    if let syn::Type::Path(type_path) = inner_type {
        if let Some(segment) = type_path.path.segments.last() {
            let type_name = segment.ident.to_string();
            match type_name.as_str() {
                "String" | "str" => return "TEXT",
                "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64" | "usize" => {
                    return if cfg!(feature = "postgres") {
                        "BIGINT"
                    } else {
                        "INTEGER"
                    };
                }
                "f32" | "f64" => {
                    return if cfg!(feature = "postgres") {
                        "DOUBLE PRECISION"
                    } else {
                        "REAL"
                    };
                }
                "bool" => {
                    return if cfg!(feature = "postgres") {
                        "BOOLEAN"
                    } else {
                        "INTEGER"
                    };
                }
                "Vec" => {
                    if is_byte_vec_type(inner_type) {
                        return if cfg!(feature = "postgres") {
                            "BYTEA"
                        } else {
                            "BLOB"
                        };
                    }
                    return if cfg!(feature = "postgres") {
                        "JSONB"
                    } else {
                        "TEXT"
                    };
                }
                "Uuid" => {
                    return if cfg!(feature = "postgres") {
                        "UUID"
                    } else {
                        "TEXT"
                    };
                }
                "DateTime" => {
                    return if cfg!(feature = "postgres") {
                        "TIMESTAMPTZ"
                    } else {
                        "TEXT"
                    };
                }
                _ => return "TEXT",
            }
        }
    }

    "TEXT"
}
