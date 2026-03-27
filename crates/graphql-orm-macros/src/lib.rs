//! Procedural macros for GraphQL/ORM backends.
//!
//! This crate provides macros to reduce boilerplate for an `async-graphql` +
//! SQLx-style backend:
//!
//! - `mutation_result!` - Generate GraphQL mutation result types
//! - `#[derive(GraphQLEntity)]` - Generate GraphQL types, filters, and SQL helpers from a struct
//! - `#[derive(GraphQLRelations)]` - Generate relation loading with look_ahead support
//! - `#[derive(GraphQLOperations)]` - Generate Query/Mutation/Subscription structs

use convert_case::{Case, Casing};
use proc_macro::TokenStream;
use quote::quote;
use syn::punctuated::Punctuated;
use syn::{
    Data, DeriveInput, Field, Fields, Ident, Meta, Token, parse::Parse, parse::ParseStream,
    parse_macro_input,
};

#[cfg(not(any(
    feature = "sqlite",
    feature = "postgres",
    feature = "mysql",
    feature = "mssql"
)))]
compile_error!("Enable exactly one database backend feature: sqlite, postgres, mysql, or mssql.");

#[cfg(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mysql"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mysql"),
    all(feature = "postgres", feature = "mssql"),
    all(feature = "mysql", feature = "mssql")
))]
compile_error!("Enable only one database backend feature at a time.");

fn backend_row_type_tokens() -> proc_macro2::TokenStream {
    if cfg!(feature = "sqlite") {
        quote! { ::graphql_orm::DbRow }
    } else if cfg!(feature = "postgres") {
        quote! { ::graphql_orm::DbRow }
    } else if cfg!(feature = "mysql") {
        quote! { ::graphql_orm::DbRow }
    } else if cfg!(feature = "mssql") {
        quote! { ::graphql_orm::DbRow }
    } else {
        proc_macro2::TokenStream::new()
    }
}

fn backend_pool_type_tokens() -> proc_macro2::TokenStream {
    if cfg!(feature = "sqlite") {
        quote! { ::graphql_orm::DbPool }
    } else if cfg!(feature = "postgres") {
        quote! { ::graphql_orm::DbPool }
    } else if cfg!(feature = "mysql") {
        quote! { ::graphql_orm::DbPool }
    } else if cfg!(feature = "mssql") {
        quote! { ::graphql_orm::DbPool }
    } else {
        proc_macro2::TokenStream::new()
    }
}

fn backend_helper_import_tokens() -> proc_macro2::TokenStream {
    if cfg!(feature = "sqlite") {
        quote! { use ::graphql_orm::db::sqlite_helpers::*; }
    } else if cfg!(feature = "postgres") {
        quote! { use ::graphql_orm::db::postgres_helpers::*; }
    } else if cfg!(feature = "mysql") {
        quote! { use ::graphql_orm::db::mysql_helpers::*; }
    } else if cfg!(feature = "mssql") {
        quote! { use ::graphql_orm::db::mssql_helpers::*; }
    } else {
        proc_macro2::TokenStream::new()
    }
}

fn backend_current_epoch_expr() -> &'static str {
    if cfg!(feature = "postgres") {
        "(EXTRACT(EPOCH FROM NOW())::bigint)"
    } else {
        "(unixepoch())"
    }
}

/// Generate a GraphQL mutation result type with success, error, and optional entity field.
///
/// # Usage
///
/// ```ignore
/// // Simple result (success + error only)
/// mutation_result!(MutationResult);
///
/// // With entity field
/// mutation_result!(CreateResult, entity: Entity);
/// mutation_result!(DeleteResult, record: ::graphql_orm::types::Record);
/// mutation_result!(BatchResult, items: Vec<Entity>);
/// ```
///
/// # Generated Code
///
/// For `mutation_result!(CreateResult, entity: Entity)`:
///
/// ```ignore
/// #[derive(Debug, Clone, ::graphql_orm::async_graphql::SimpleObject)]
/// pub struct CreateResult {
///     pub success: bool,
///     pub error: Option<String>,
///     pub entity: Option<Entity>,
/// }
///
/// impl CreateResult {
///     pub fn ok(entity: Entity) -> Self {
///         Self { success: true, error: None, entity: Some(entity) }
///     }
///     pub fn err(msg: impl Into<String>) -> Self {
///         Self { success: false, error: Some(msg.into()), entity: None }
///     }
/// }
/// ```
#[proc_macro]
pub fn mutation_result(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as MutationResultInput);

    let struct_name = &parsed.name;

    if let Some((field_name, field_type)) = parsed.field {
        // Result with entity field
        let output = quote! {
            #[derive(Debug, Clone, ::graphql_orm::async_graphql::SimpleObject)]
            pub struct #struct_name {
                pub success: bool,
                pub error: Option<String>,
                pub #field_name: Option<#field_type>,
            }

            impl #struct_name {
                pub fn ok(#field_name: #field_type) -> Self {
                    Self {
                        success: true,
                        error: None,
                        #field_name: Some(#field_name),
                    }
                }

                pub fn err(msg: impl Into<String>) -> Self {
                    Self {
                        success: false,
                        error: Some(msg.into()),
                        #field_name: None,
                    }
                }
            }
        };
        output.into()
    } else {
        // Simple result (no entity field)
        let output = quote! {
            #[derive(Debug, Clone, ::graphql_orm::async_graphql::SimpleObject)]
            pub struct #struct_name {
                pub success: bool,
                pub error: Option<String>,
            }

            impl #struct_name {
                pub fn ok() -> Self {
                    Self {
                        success: true,
                        error: None,
                    }
                }

                pub fn err(msg: impl Into<String>) -> Self {
                    Self {
                        success: false,
                        error: Some(msg.into()),
                    }
                }
            }
        };
        output.into()
    }
}

/// Input for mutation_result! macro
struct MutationResultInput {
    name: Ident,
    field: Option<(Ident, syn::Type)>,
}

impl Parse for MutationResultInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name: Ident = input.parse()?;

        let field = if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            let field_name: Ident = input.parse()?;
            input.parse::<Token![:]>()?;
            let field_type: syn::Type = input.parse()?;
            Some((field_name, field_type))
        } else {
            None
        };

        Ok(MutationResultInput { name, field })
    }
}

// ============================================================================
// GraphQLEntity Derive Macro
// ============================================================================

/// Derive macro for generating GraphQL types, filters, and SQL helpers.
///
/// # Attributes
///
/// ## Struct-level:
/// - `#[graphql_entity(table = "...", plural = "...", default_sort = "...")]`
/// - `#[graphql_entity(unique_composite = "col_a,col_b")]` (repeatable)
///
/// ## Field-level:
/// - `#[primary_key]` - Mark as primary key column
/// - `#[filterable(type = "string|number|boolean|date")]` - Enable filtering
/// - `#[sortable]` - Enable sorting by this field
/// - `#[unique]` - Enforce DB uniqueness for this column
/// - `#[db_column = "..."]` - Map to different column name
/// - `#[relation(...)]` - Define a relation (handled by GraphQLRelations)
/// - `#[skip_db]` - Skip this field in database operations
///
/// # Generated Code
///
/// For a struct `Entity`, generates:
/// - `EntityWhereInput` - GraphQL input for filtering
/// - `EntityOrderByInput` - GraphQL input for sorting
/// - `impl DatabaseEntity for Entity`
/// - `impl FromSqlRow for Entity`
/// - `impl DatabaseFilter for EntityWhereInput`
#[proc_macro_derive(
    GraphQLEntity,
    attributes(
        graphql_entity,
        graphql,
        graphql_orm,
        serde,
        primary_key,
        filterable,
        sortable,
        unique,
        db_column,
        relation,
        skip_db,
        date_field,
        boolean_field,
        json_field,
        transform,
        input_only,
        index,
        unique_index
    )
)]
pub fn derive_graphql_entity(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match generate_graphql_entity(&input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Derive macro for generating relation loading with look_ahead support.
#[proc_macro_derive(
    GraphQLRelations,
    attributes(graphql_entity, graphql, graphql_orm, serde, relation, transform, input_only, unique)
)]
pub fn derive_graphql_relations(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match generate_graphql_relations(&input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

// ============================================================================
// Entity Metadata Parsing
// ============================================================================

#[derive(Default)]
struct EntityMetadata {
    table_name: Option<String>,
    plural_name: Option<String>,
    default_sort: Option<String>,
    /// Optional async hook path invoked after create/update/delete mutations.
    notify_handler: Option<String>,
    unique_composite: Vec<Vec<String>>,
    indexes: Vec<(bool, Vec<String>)>,
    serde_rename_all: Option<String>,
}

fn parse_entity_metadata(attrs: &[syn::Attribute]) -> syn::Result<EntityMetadata> {
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

fn has_graphql_complex(attrs: &[syn::Attribute]) -> bool {
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
struct FieldMetadata {
    graphql_name: Option<String>,
    serde_name: Option<String>,
    db_column: Option<String>,
    filterable: Option<String>,
    sortable: bool,
    unique: bool,
    is_primary_key: bool,
    is_relation: bool,
    relation_target: Option<String>,
    relation_from: Option<String>,
    relation_to: Option<String>,
    relation_multiple: bool,
    skip_db: bool,
    /// Skip from Create/Update inputs only (e.g. password_hash); field remains in DB and struct
    skip_input: bool,
    is_date_field: bool,
    is_boolean_field: bool,
    is_json_field: bool,
    /// Async write transform: fn(&Context, String) -> Result<String>
    /// Applied before INSERT/UPDATE to transform the value (e.g., encryption)
    transform_write: Option<String>,
    /// Sync read transform: fn(T) -> T
    /// Applied after reading from the database row (e.g., decryption)
    transform_read: Option<String>,
    /// If true, include in Create/Update inputs even if #[graphql(skip)] is set.
    /// Useful for fields that should be writable but never exposed in queries.
    input_only: bool,
    read: bool,
    write: bool,
    filter: bool,
    order: bool,
    subscribe: bool,
    read_policy: Option<String>,
    write_policy: Option<String>,
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
struct ParsedField {
    field: Field,
    meta: FieldMetadata,
}

fn parse_field_metadata(field: &Field) -> syn::Result<FieldMetadata> {
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
                            meta.write = false;
                            meta.filter = false;
                            meta.order = false;
                            meta.subscribe = false;
                            meta.skip_input = true;
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

fn collect_parsed_fields<'a>(
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

fn graphql_field_name(meta: &FieldMetadata, rust_name: &str, rename_all: Option<&str>) -> String {
    if let Some(graphql_name) = &meta.graphql_name {
        graphql_name.clone()
    } else if let Some(serde_name) = &meta.serde_name {
        serde_name.clone()
    } else if let Some(rule) = rename_all {
        apply_rename_rule(rust_name, rule)
    } else {
        rust_name.to_string()
    }
}

/// Wrap a value expression with an async, context-aware write transform.
///
/// The transform function must have the signature:
///   `async fn(&::graphql_orm::async_graphql::Context<'_>, String) -> ::graphql_orm::async_graphql::Result<String>`
///
/// If `transform_write` is None, returns the original expression unchanged.
fn maybe_wrap_write_transform(
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
fn maybe_wrap_read_transform(
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

fn generate_graphql_entity(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
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
    let entity_name_lit = struct_name.to_string();
    let rename_all_rule = entity_meta.serde_rename_all.as_deref();
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

    for field in fields {
        let field_name = field.ident.as_ref().unwrap();
        let field_type = &field.ty;
        let field_meta = parse_field_metadata(field)?;

        // Skip relation fields for column list
        if field_meta.is_relation || field_meta.skip_db {
            if field_meta.is_relation {
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

                relation_metadata_defs.push(quote! {
                    ::graphql_orm::graphql::orm::RelationMetadata {
                        field_name: #graphql_name,
                        target_type: #target_type,
                        source_column: #source_column,
                        target_column: #target_column,
                        is_multiple: #is_multiple,
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

    let object_impl = if legacy_graphql_complex {
        quote! {}
    } else {
        quote! {
            #[::graphql_orm::async_graphql::Object]
            impl #struct_name {
                #(#object_field_methods)*
            }
        }
    };

    Ok(quote! {
        // WhereInput for filtering
        #[derive(::graphql_orm::async_graphql::InputObject, Default, Clone, Debug)]
        #[graphql(name = #where_input_name_str)]
        pub struct #where_input_name {
            #(#where_input_fields)*

            /// Logical AND of conditions
            #[graphql(name = "And")]
            pub and: Option<Vec<#where_input_name>>,

            /// Logical OR of conditions
            #[graphql(name = "Or")]
            pub or: Option<Vec<#where_input_name>>,

            /// Logical NOT of condition
            #[graphql(name = "Not")]
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
                    ::graphql_orm::graphql::orm::EntityMetadata::from_schema::<Self>(#struct_name_str)
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

    if meta.is_json_field {
        return Ok(quote! {
            #field_name: {
                let s: String = row.try_get(#db_col)?;
                json_to_vec(&s)
            },
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

fn is_vec_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            return segment.ident == "Vec";
        }
    }
    false
}

fn is_byte_vec_type(ty: &syn::Type) -> bool {
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

fn is_option_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            return segment.ident == "Option";
        }
    }
    false
}

fn is_bool_type(ty: &syn::Type) -> bool {
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

fn type_path_last_ident(ty: &syn::Type) -> Option<&syn::Ident> {
    if let syn::Type::Path(type_path) = ty {
        return type_path.path.segments.last().map(|segment| &segment.ident);
    }

    None
}

fn option_inner_type(ty: &syn::Type) -> Option<&syn::Type> {
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

fn is_uuid_type(ty: &syn::Type) -> bool {
    if type_path_last_ident(ty).is_some_and(|ident| ident == "Uuid") {
        return true;
    }

    option_inner_type(ty).is_some_and(is_uuid_type)
}

/// Convert Rust type to the configured backend SQL type string
fn rust_type_to_sql_type(ty: &syn::Type, meta: &FieldMetadata) -> &'static str {
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

// ============================================================================
// GraphQL Relations Code Generation
// ============================================================================

/// Relation definition parsed from attributes
#[derive(Clone)]
struct RelationDef {
    field_name: syn::Ident,
    graphql_name: String,
    target_type_str: String,
    source_column: String,
    fk_column: String,
    is_multiple: bool,
    source_field_ty: syn::Type,
    source_supports_dataloader: bool,
}

#[derive(Copy, Clone)]
enum RelationValueKind {
    String,
    Uuid,
    Int,
    Float,
    Bool,
}

fn classify_relation_value_type(ty: &syn::Type) -> Option<(RelationValueKind, bool)> {
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

fn generate_graphql_relations(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let struct_name = &input.ident;
    let pool_type = backend_pool_type_tokens();

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

    let entity_meta = parse_entity_metadata(&input.attrs)?;
    let rename_all_rule = entity_meta.serde_rename_all.as_deref();
    let legacy_graphql_complex = has_graphql_complex(&input.attrs);
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

    for parsed_field in &parsed_fields {
        let field = &parsed_field.field;
        let meta = &parsed_field.meta;
        if !meta.is_relation || !meta.read {
            continue;
        }

        let field_name = field.ident.clone().unwrap();
        let rust_name = field_name.to_string();
        let graphql_name = graphql_field_name(meta, &rust_name, rename_all_rule);

        let target_type = meta
            .relation_target
            .clone()
            .unwrap_or_else(|| "Unknown".to_string());
        let to_col = meta
            .relation_to
            .clone()
            .unwrap_or_else(|| "unknown_id".to_string());
        let from_col = meta
            .relation_from
            .clone()
            .unwrap_or_else(|| pk_field.to_string());
        let source_field = parsed_fields
            .iter()
            .find(|parsed| {
                parsed
                    .field
                    .ident
                    .as_ref()
                    .map(|ident| ident == &syn::Ident::new(&from_col, ident.span()))
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
        let source_supports_dataloader = matches!(
            classify_relation_value_type(&source_field_ty),
            Some((RelationValueKind::String, _))
        );
        let is_multiple = meta.relation_multiple;

        relations.push(RelationDef {
            field_name,
            graphql_name,
            target_type_str: target_type,
            source_column: from_col,
            fk_column: to_col,
            is_multiple,
            source_field_ty,
            source_supports_dataloader,
        });
    }

    // Generate relation metadata
    let relation_metadata: Vec<_> = relations
        .iter()
        .map(|r| {
            let graphql_name = &r.graphql_name;
            let target_type = &r.target_type_str;
            let source_column = &r.source_column;
            let target_column = &r.fk_column;
            let is_multiple = r.is_multiple;
            quote! {
                ::graphql_orm::graphql::orm::RelationMetadata {
                    field_name: #graphql_name,
                    target_type: #target_type,
                    source_column: #source_column,
                    target_column: #target_column,
                    is_multiple: #is_multiple,
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
        let fk_column = &r.fk_column;
        let source_column = &r.source_column;
        let source_field = syn::Ident::new(source_column, struct_name.span());
        let source_supports_dataloader = r.source_supports_dataloader;
        let source_ty = &r.source_field_ty;

        let (source_kind, source_is_option) =
            classify_relation_value_type(source_ty).ok_or_else(|| {
                syn::Error::new_spanned(
                    source_ty,
                    format!(
                        "Unsupported relation source field type for '{}.{}': expected String/uuid/int/float/bool (optionals allowed)",
                        struct_name, field_name
                    ),
                )
            })?;

        let sql_value_expr = match source_kind {
            RelationValueKind::String => quote! { ::graphql_orm::graphql::orm::SqlValue::String(value.clone()) },
            RelationValueKind::Uuid => quote! { ::graphql_orm::graphql::orm::SqlValue::Uuid(*value) },
            RelationValueKind::Int => quote! { ::graphql_orm::graphql::orm::SqlValue::Int(*value as i64) },
            RelationValueKind::Float => quote! { ::graphql_orm::graphql::orm::SqlValue::Float((*value).into()) },
            RelationValueKind::Bool => quote! { ::graphql_orm::graphql::orm::SqlValue::Bool(*value) },
        };

        let loader_key_expr = match source_kind {
            RelationValueKind::String => quote! { value.clone() },
            RelationValueKind::Uuid => quote! { value.to_string() },
            _ => quote! { value.to_string() },
        };

        // Generate type name strings for use in fully-qualified paths
        let target_type_str = &r.target_type_str;
        let where_input_str = format!("{}WhereInput", r.target_type_str);
        let order_by_input_str = format!("{}OrderByInput", r.target_type_str);
        let connection_type_str = format!("{}Connection", r.target_type_str);
        let edge_type_str = format!("{}Edge", r.target_type_str);

        // Create idents for local use in the macro
        let target_type = syn::Ident::new(target_type_str, struct_name.span());
        let where_input = syn::Ident::new(&where_input_str, struct_name.span());
        let order_by_input = syn::Ident::new(&order_by_input_str, struct_name.span());
        let connection_type = syn::Ident::new(&connection_type_str, struct_name.span());
        let edge_type = syn::Ident::new(&edge_type_str, struct_name.span());

        let source_binding_multiple = if source_is_option {
            quote! {
                let Some(value) = self.#source_field.as_ref() else {
                    let page_info = ::graphql_orm::graphql::pagination::PageInfo {
                        has_next_page: false,
                        has_previous_page: false,
                        start_cursor: None,
                        end_cursor: None,
                        total_count: Some(0),
                    };
                    return Ok(#connection_type { edges: Vec::new(), page_info });
                };
                let relation_sql_value = #sql_value_expr;
                let relation_loader_key = #loader_key_expr;
            }
        } else {
            quote! {
                let value = &self.#source_field;
                let relation_sql_value = #sql_value_expr;
                let relation_loader_key = #loader_key_expr;
            }
        };

        let source_binding_single = if source_is_option {
            quote! {
                let Some(value) = self.#source_field.as_ref() else {
                    return Ok(None);
                };
                let relation_sql_value = #sql_value_expr;
                let relation_loader_key = #loader_key_expr;
            }
        } else {
            quote! {
                let value = &self.#source_field;
                let relation_sql_value = #sql_value_expr;
                let relation_loader_key = #loader_key_expr;
            }
        };

        let relation_query_key = quote! {
            ::graphql_orm::graphql::loaders::RelationQueryKey {
                relation: #graphql_name,
                parent_key: relation_loader_key.clone(),
                parent_value: relation_sql_value.clone(),
                fk_column: #fk_column,
                where_signature: where_input
                    .as_ref()
                    .and_then(|filter| filter.to_filter_expression().map(|expr| format!("{expr:?}"))),
                order_signature: order_by
                    .as_ref()
                    .and_then(|order| order.to_sort_expression().map(|expr| expr.clause)),
                page_signature: page
                    .as_ref()
                    .map(|page| format!("limit={:?};offset={}", page.limit(), page.offset())),
                filter: where_input.as_ref().and_then(|filter| filter.to_filter_expression()),
                sorts: order_by
                    .as_ref()
                    .and_then(|order| order.to_sort_expression())
                    .into_iter()
                    .collect(),
                pagination: page.as_ref().map(::graphql_orm::graphql::orm::PaginationRequest::from),
            }
        };

        let single_relation_query_key = quote! {
            ::graphql_orm::graphql::loaders::RelationQueryKey {
                relation: #graphql_name,
                parent_key: relation_loader_key.clone(),
                parent_value: relation_sql_value.clone(),
                fk_column: #fk_column,
                where_signature: None,
                order_signature: None,
                page_signature: None,
                filter: None,
                sorts: Vec::new(),
                pagination: None,
            }
        };

        if r.is_multiple {
            // One-to-many relation with smart batching
            Ok(quote! {
                /// Get related #graphql_name with optional filtering, sorting, and pagination.
                ///
                /// When no arguments are provided, uses DataLoader to batch queries and
                /// avoid N+1 when loading relations for multiple parent entities.
                /// When filter/sort/pagination arguments are provided, uses direct
                /// database query for full SQL support.
                #[graphql(name = #graphql_name)]
                async fn #field_name(
                    &self,
                    ctx: &::graphql_orm::async_graphql::Context<'_>,
                    #[graphql(name = "Where")] where_input: Option<#where_input>,
                    #[graphql(name = "OrderBy")] order_by: Option<#order_by_input>,
                    #[graphql(name = "Page")] page: Option<::graphql_orm::graphql::orm::PageInput>,
                ) -> ::graphql_orm::async_graphql::Result<#connection_type> {
                    use ::graphql_orm::graphql::orm::{DatabaseEntity, DatabaseFilter, DatabaseOrderBy, EntityQuery, SqlValue};

                    let db = ctx.data_unchecked::<::graphql_orm::db::Database>();

                    if where_input.is_none() && order_by.is_none() && page.is_none() && !self.#field_name.is_empty() {
                        let entities = self.#field_name.clone();
                        let edges: Vec<#edge_type> = entities
                            .into_iter()
                            .enumerate()
                            .map(|(i, entity)| #edge_type {
                                cursor: ::graphql_orm::graphql::pagination::encode_cursor(i as i64),
                                node: entity,
                            })
                            .collect();
                        let page_info = ::graphql_orm::graphql::pagination::PageInfo {
                            has_next_page: false,
                            has_previous_page: false,
                            start_cursor: edges.first().map(|e| e.cursor.clone()),
                            end_cursor: edges.last().map(|e| e.cursor.clone()),
                            total_count: Some(edges.len() as i64),
                        };
                        return Ok(#connection_type { edges, page_info });
                    }

                    // Use DataLoader whenever the relation key can be batched.
                    #source_binding_multiple

                    let use_dataloader = #source_supports_dataloader;

                    let loaded = if use_dataloader {
                        use ::graphql_orm::graphql::loaders::RelationLoader;
                        use ::graphql_orm::async_graphql::dataloader::DataLoader;

                        let loader = ctx.data_unchecked::<DataLoader<RelationLoader<#target_type>>>();
                        loader
                            .load_one(#relation_query_key)
                            .await
                            .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?
                            .unwrap_or(::graphql_orm::graphql::loaders::RelationLoadResult {
                                entities: Vec::new(),
                                total_count: 0,
                                has_next_page: false,
                                has_previous_page: false,
                                offset: 0,
                            })
                    } else {
                        // Slow path: Use direct query with full SQL support
                        let mut query = EntityQuery::<#target_type>::new()
                            .where_clause(
                                &format!("{} = {}", #fk_column, #target_type::__gom_placeholder(1)),
                                relation_sql_value
                            );

                        if let Some(ref filter) = where_input {
                            query = query.filter(filter);
                        }

                        if let Some(ref order) = order_by {
                            query = query.order_by(order);
                        }

                        if query.order_clauses.is_empty() {
                            query = query.default_order();
                        }

                        if let Some(ref p) = page {
                            query = query.paginate(p);
                        }

                        // Count must be computed before/independent of pagination window.
                        // EntityQuery::count ignores limit/offset and uses only WHERE clauses.
                        let total = query
                            .count(db)
                            .await
                            .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;

                        let offset = page.as_ref().map(|p| p.offset()).unwrap_or(0) as usize;

                        let entities = query.fetch_all(db)
                            .await
                            .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;

                        ::graphql_orm::graphql::loaders::RelationLoadResult {
                            has_next_page: (offset as i64 + entities.len() as i64) < total,
                            has_previous_page: offset > 0,
                            entities,
                            total_count: total,
                            offset: offset as i64,
                        }
                    };

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
                /// Get related #graphql_name
                #[graphql(name = #graphql_name)]
                async fn #field_name(
                    &self,
                    ctx: &::graphql_orm::async_graphql::Context<'_>,
                ) -> ::graphql_orm::async_graphql::Result<Option<#target_type>> {
                    use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, SqlValue};

                    if self.#field_name.is_some() {
                        return Ok(self.#field_name.clone());
                    }

                    let db = ctx.data_unchecked::<::graphql_orm::db::Database>();
                    #source_binding_single

                    let result = if #source_supports_dataloader {
                        use ::graphql_orm::graphql::loaders::RelationLoader;
                        use ::graphql_orm::async_graphql::dataloader::DataLoader;

                        let loader = ctx.data_unchecked::<DataLoader<RelationLoader<#target_type>>>();
                        loader
                            .load_one(#single_relation_query_key)
                            .await
                            .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?
                            .and_then(|mut result| result.entities.drain(..).next())
                    } else {
                        EntityQuery::<#target_type>::new()
                            .where_clause(
                                &format!("{} = {}", #fk_column, #target_type::__gom_placeholder(1)),
                                relation_sql_value
                            )
                            .fetch_one(db)
                            .await
                            .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?
                    };

                    Ok(result)
                }
            })
        }
    })
    .collect::<syn::Result<Vec<_>>>()?;

    let bulk_load_blocks: Vec<proc_macro2::TokenStream> = relations
        .iter()
        .map(|r| -> syn::Result<proc_macro2::TokenStream> {
            let field_name = &r.field_name;
            let graphql_name = &r.graphql_name;
            let fk_column = &r.fk_column;
            let source_column = &r.source_column;
            let source_field = syn::Ident::new(source_column, struct_name.span());
            let target_type = syn::Ident::new(&r.target_type_str, struct_name.span());
            let source_ty = &r.source_field_ty;

            let (source_kind, source_is_option) =
                classify_relation_value_type(source_ty).ok_or_else(|| {
                    syn::Error::new_spanned(
                        source_ty,
                        format!(
                        "Unsupported relation source field type for '{}.{}': expected String/uuid/int/float/bool (optionals allowed)",
                            struct_name, field_name
                        ),
                    )
                })?;

            let key_string_expr = match source_kind {
                RelationValueKind::String => quote! { value.clone() },
                RelationValueKind::Uuid => quote! { value.to_string() },
                _ => quote! { value.to_string() },
            };
            let sql_value_expr = match source_kind {
                RelationValueKind::String => {
                    quote! { ::graphql_orm::graphql::orm::SqlValue::String(value.clone()) }
                }
                RelationValueKind::Uuid => {
                    quote! { ::graphql_orm::graphql::orm::SqlValue::Uuid(*value) }
                }
                RelationValueKind::Int => {
                    quote! { ::graphql_orm::graphql::orm::SqlValue::Int((*value) as i64) }
                }
                RelationValueKind::Float => {
                    quote! { ::graphql_orm::graphql::orm::SqlValue::Float((*value).into()) }
                }
                RelationValueKind::Bool => {
                    quote! { ::graphql_orm::graphql::orm::SqlValue::Bool(*value) }
                }
            };

            let entity_key_pair_expr = if source_is_option {
                quote! {
                    entity.#source_field.as_ref().map(|value| {
                        (#key_string_expr, #sql_value_expr)
                    })
                }
            } else {
                quote! {
                    {
                        let value = &entity.#source_field;
                        Some((#key_string_expr, #sql_value_expr))
                    }
                }
            };

            let assign_expr = if source_is_option {
                if r.is_multiple {
                    quote! {
                        if let Some(value) = entity.#source_field.as_ref() {
                            let value = value;
                            let relation_key = #key_string_expr;
                            entity.#field_name = grouped.remove(&relation_key).unwrap_or_default();
                        } else {
                            entity.#field_name = Vec::new();
                        }
                    }
                } else {
                    quote! {
                        if let Some(value) = entity.#source_field.as_ref() {
                            let value = value;
                            let relation_key = #key_string_expr;
                            entity.#field_name = grouped.remove(&relation_key);
                        } else {
                            entity.#field_name = None;
                        }
                    }
                }
            } else if r.is_multiple {
                quote! {
                    let value = &entity.#source_field;
                    let relation_key = #key_string_expr;
                    entity.#field_name = grouped.remove(&relation_key).unwrap_or_default();
                }
            } else {
                quote! {
                    let value = &entity.#source_field;
                    let relation_key = #key_string_expr;
                    entity.#field_name = grouped.remove(&relation_key);
                }
            };

            let grouped_type = if r.is_multiple {
                quote! { std::collections::HashMap<String, Vec<#target_type>> }
            } else {
                quote! { std::collections::HashMap<String, #target_type> }
            };

            let insert_grouped = if r.is_multiple {
                quote! {
                    grouped.entry(relation_key).or_default().push(related);
                }
            } else {
                quote! {
                    grouped.entry(relation_key).or_insert(related);
                }
            };

            Ok(quote! {
                if Self::__gom_selection_contains(selection, #graphql_name)
                    && !Self::__gom_selected_field_has_arguments(selection, #graphql_name)
                {
                    let mut unique_relation_keys: Vec<(String, ::graphql_orm::graphql::orm::SqlValue)> = Vec::new();
                    let mut seen_relation_keys = std::collections::HashSet::new();

                    for entity in entities.iter() {
                        if let Some((relation_key, relation_value)) = #entity_key_pair_expr {
                            if seen_relation_keys.insert(relation_key.clone()) {
                                unique_relation_keys.push((relation_key, relation_value));
                            }
                        }
                    }

                    let mut grouped: #grouped_type = std::collections::HashMap::new();

                    if !unique_relation_keys.is_empty() {
                        let bind_values = unique_relation_keys
                            .iter()
                            .map(|(_, value)| value.clone())
                            .collect::<Vec<_>>();
                        let placeholders = (0..unique_relation_keys.len())
                            .map(|index| <#target_type>::__gom_placeholder(index + 1))
                            .collect::<Vec<_>>();
                        let sql = format!(
                            "SELECT {}, CAST({} AS TEXT) AS __gom_relation_key FROM {} WHERE {} IN ({})",
                            <#target_type as ::graphql_orm::graphql::orm::DatabaseEntity>::column_names().join(", "),
                            #fk_column,
                            <#target_type as ::graphql_orm::graphql::orm::DatabaseEntity>::TABLE_NAME,
                            #fk_column,
                            placeholders.join(", ")
                        );

                        let rows = ::graphql_orm::graphql::orm::fetch_rows(pool, &sql, &bind_values).await?;
                        for row in rows {
                            use ::graphql_orm::sqlx::Row;
                            let relation_key: String = row.try_get("__gom_relation_key")?;
                            let related = <#target_type as ::graphql_orm::graphql::orm::FromSqlRow>::from_row(&row)?;
                            #insert_grouped
                        }
                    }

                    for entity in entities.iter_mut() {
                        #assign_expr
                    }
                }
            })
        })
        .collect::<syn::Result<Vec<_>>>()?;

    let struct_name_str = struct_name.to_string();
    let has_relations = !relations.is_empty();

    let complex_object_impl = if has_relations {
        if legacy_graphql_complex {
            quote! {
                #[::graphql_orm::async_graphql::ComplexObject]
                impl #struct_name {
                    #(#relation_resolvers)*
                }
            }
        } else {
        quote! {
            #[::graphql_orm::async_graphql::Object(extends)]
            impl #struct_name {
                #(#relation_resolvers)*
            }
        }
        }
    } else {
        quote! {}
    };

    Ok(quote! {
        impl ::graphql_orm::graphql::orm::RelationLoader for #struct_name {
            async fn load_relations(
                &mut self,
                pool: &#pool_type,
                selection: &[::graphql_orm::async_graphql::context::SelectionField<'_>],
            ) -> Result<(), ::graphql_orm::sqlx::Error> {
                Self::bulk_load_relations(std::slice::from_mut(self), pool, selection).await
            }

            async fn bulk_load_relations(
                entities: &mut [Self],
                pool: &#pool_type,
                selection: &[::graphql_orm::async_graphql::context::SelectionField<'_>],
            ) -> Result<(), ::graphql_orm::sqlx::Error> {
                #(#bulk_load_blocks)*
                Ok(())
            }
        }

        impl #struct_name {
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
                static RELATIONS: &[::graphql_orm::graphql::orm::RelationMetadata] = &[
                    #(#relation_metadata),*
                ];
                RELATIONS
            }

            /// Get entity name for relation registry
            pub fn entity_name() -> &'static str {
                #struct_name_str
            }
        }

        #complex_object_impl
    })
}

// ============================================================================
// GraphQLOperations Derive Macro
// ============================================================================

/// Derive macro for generating Query, Mutation, and Subscription structs.
///
/// This macro generates complete CRUD operations for a GraphQL entity:
///
/// # Generated Queries
/// - `{PluralName}(Where, OrderBy, Page)` - List with filtering, sorting, pagination
/// - `{EntityName}(Id)` - Get single entity by ID
///
/// # Generated Mutations  
/// - `Create{EntityName}(Input)` - Create new entity
/// - `Update{EntityName}(Id, Input)` - Update existing entity
/// - `Delete{EntityName}(Id)` - Delete single entity
/// - `Delete{PluralName}(Where)` - Delete multiple entities matching Where filter
///
/// # Generated Subscriptions
/// - `{EntityName}Changed(Filter)` - Real-time updates
///
/// # Usage
///
/// ```ignore
/// #[derive(GraphQLEntity, GraphQLRelations, GraphQLOperations)]
/// #[graphql_entity(table = "entities", plural = "Entities")]
/// pub struct Entity { ... }
/// ```
///
/// Then merge into your schema:
/// ```ignore
/// #[derive(MergedObject)]
/// pub struct QueryRoot(EntityQueries, RelatedEntityQueries, ...);
/// ```
#[proc_macro_derive(
    GraphQLOperations,
    attributes(graphql_entity, graphql, graphql_orm, serde, primary_key, transform, input_only, unique)
)]
pub fn derive_graphql_operations(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match generate_graphql_operations(&input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn generate_graphql_operations(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let struct_name = &input.ident;
    let struct_name_str = struct_name.to_string();
    let pool_type = backend_pool_type_tokens();

    let data = match &input.data {
        Data::Struct(data) => data,
        _ => {
            return Err(syn::Error::new_spanned(
                input,
                "GraphQLOperations can only be derived for structs",
            ));
        }
    };

    let fields = match &data.fields {
        Fields::Named(fields) => &fields.named,
        _ => {
            return Err(syn::Error::new_spanned(
                input,
                "GraphQLOperations requires named fields",
            ));
        }
    };

    let entity_meta = parse_entity_metadata(&input.attrs)?;
    let rename_all_rule = entity_meta.serde_rename_all.as_deref();
    let table_name = entity_meta.table_name.as_deref().unwrap_or("unknown");
    let plural_name = entity_meta
        .plural_name
        .clone()
        .unwrap_or_else(|| format!("{}s", struct_name));

    // Generate optional post-mutation hook code if a hook path is configured.
    //
    // Expected signature:
    // `async fn(&::graphql_orm::async_graphql::Context<'_>, &#pool_type, &#Entity, ChangeAction)
    //      -> ::graphql_orm::async_graphql::Result<()>`
    let notify_handler_path = if let Some(ref notify_handler) = entity_meta.notify_handler {
        Some(syn::parse_str::<syn::Path>(notify_handler).map_err(|_| {
            syn::Error::new(
                struct_name.span(),
                "graphql_entity notify/notify_with must be a valid Rust path string",
            )
        })?)
    } else {
        None
    };
    let notify_on_created = if let Some(ref notify_handler) = notify_handler_path {
        quote! {
            #notify_handler(ctx, pool, &entity, ::graphql_orm::graphql::orm::ChangeAction::Created).await?;
        }
    } else {
        quote! {}
    };
    let notify_on_updated = if let Some(ref notify_handler) = notify_handler_path {
        quote! {
            #notify_handler(ctx, pool, &entity, ::graphql_orm::graphql::orm::ChangeAction::Updated).await?;
        }
    } else {
        quote! {}
    };
    let notify_on_deleted = if let Some(ref notify_handler) = notify_handler_path {
        quote! {
            #notify_handler(ctx, pool, &entity, ::graphql_orm::graphql::orm::ChangeAction::Deleted).await?;
        }
    } else {
        quote! {}
    };
    let entity_name_lit = struct_name_str.clone();

    // Find primary key field
    let mut pk_field_name: Option<syn::Ident> = None;
    let mut pk_type_ty: Option<syn::Type> = None;
    for field in fields {
        let meta = parse_field_metadata(field)?;
        if meta.is_primary_key {
            pk_field_name = Some(field.ident.clone().unwrap());
            pk_type_ty = Some(field.ty.clone());
            break;
        }
    }
    let pk_field = pk_field_name
        .clone()
        .unwrap_or_else(|| syn::Ident::new("id", struct_name.span()));
    let pk_type_ty: syn::Type = pk_type_ty.unwrap_or_else(|| syn::parse_quote!(String));
    let pk_type = quote! { #pk_type_ty };
    let auto_generated_pk = pk_field == syn::Ident::new("id", pk_field.span());
    let pk_is_uuid = is_uuid_type(&pk_type_ty);
    let pk_bind_value = if pk_is_uuid {
        quote! { ::graphql_orm::graphql::orm::SqlValue::Uuid(id) }
    } else {
        quote! { ::graphql_orm::graphql::orm::SqlValue::String(id.to_string()) }
    };
    let pk_bind_value_ref = if pk_is_uuid {
        quote! { ::graphql_orm::graphql::orm::SqlValue::Uuid(*id) }
    } else {
        quote! { ::graphql_orm::graphql::orm::SqlValue::String(id.to_string()) }
    };
    let created_pk_value = if pk_is_uuid {
        quote! { ::graphql_orm::graphql::orm::SqlValue::Uuid(created_pk) }
    } else {
        quote! { ::graphql_orm::graphql::orm::SqlValue::String(created_pk.clone()) }
    };
    let created_pk_id_string = quote! { created_pk.to_string() };

    // Generate type names
    let queries_struct = syn::Ident::new(&format!("{}Queries", struct_name), struct_name.span());
    let mutations_struct =
        syn::Ident::new(&format!("{}Mutations", struct_name), struct_name.span());
    let subscriptions_struct =
        syn::Ident::new(&format!("{}Subscriptions", struct_name), struct_name.span());
    let where_input = syn::Ident::new(&format!("{}WhereInput", struct_name), struct_name.span());
    let order_by_input =
        syn::Ident::new(&format!("{}OrderByInput", struct_name), struct_name.span());
    let create_input = syn::Ident::new(&format!("Create{}Input", struct_name), struct_name.span());
    let update_input = syn::Ident::new(&format!("Update{}Input", struct_name), struct_name.span());
    let result_type = syn::Ident::new(&format!("{}Result", struct_name), struct_name.span());
    let changed_event =
        syn::Ident::new(&format!("{}ChangedEvent", struct_name), struct_name.span());

    // GraphQL operation names (PascalCase)
    let list_query_name = &plural_name;
    let single_query_name = &struct_name_str;
    let create_mutation_name = format!("Create{}", struct_name);
    let update_mutation_name = format!("Update{}", struct_name);
    let update_many_mutation_name = format!("Update{}", plural_name);
    let delete_mutation_name = format!("Delete{}", struct_name);
    let delete_many_mutation_name = format!("Delete{}", plural_name);
    let subscription_name = format!("{}Changed", struct_name);
    let update_many_result_type =
        syn::Ident::new(&format!("Update{}Result", plural_name), struct_name.span());
    let update_many_result_type_str = format!("Update{}Result", plural_name);
    let delete_many_result_type =
        syn::Ident::new(&format!("Delete{}Result", plural_name), struct_name.span());
    let delete_many_result_type_str = format!("Delete{}Result", plural_name);

    // Generate input fields (excluding primary key for create, all optional for update)
    let mut create_input_fields: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut update_input_fields: Vec<proc_macro2::TokenStream> = Vec::new();

    // For SQL generation
    let mut insert_columns: Vec<String> = Vec::new();
    let mut insert_binds: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut update_field_checks: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut create_policy_checks: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut update_policy_checks: Vec<proc_macro2::TokenStream> = Vec::new();

    // Track string-filterable fields for search_similar
    let mut string_filterable_fields: Vec<(syn::Ident, bool)> = Vec::new(); // (field_name, is_option)

    for field in fields {
        let field_name = field.ident.as_ref().unwrap();
        let field_type = &field.ty;
        let meta = parse_field_metadata(field)?;

        // Skip relations and computed fields
        if meta.is_relation || meta.skip_db {
            continue;
        }

        let rust_name = field_name.to_string();
        let graphql_name = graphql_field_name(&meta, &rust_name, rename_all_rule);
        let db_col = meta.db_column.clone().unwrap_or_else(|| rust_name.clone());

        // Track string-filterable fields for fuzzy search
        if meta.filter && meta.filterable.as_deref() == Some("string") {
            string_filterable_fields.push((field_name.clone(), is_option_type(field_type)));
        }

        // Skip auto-generated primary key, timestamps, and skip_input fields (e.g. password_hash)
        // But #[input_only] overrides skip_input (allows write-only fields like encrypted credentials)
        let is_timestamp = rust_name == "created_at" || rust_name == "updated_at";
        let include_in_create = (!meta.is_primary_key || !auto_generated_pk)
            && !is_timestamp
            && meta.write
            && (!meta.skip_input || meta.input_only);
        if include_in_create {
            let create_policy_check = if let Some(policy_key) = &meta.write_policy {
                quote! {
                    db.ensure_writable_field(
                        ctx,
                        #entity_name_lit,
                        #graphql_name,
                        Some(#policy_key),
                        None,
                        Some(&input.#field_name as &(dyn ::std::any::Any + Send + Sync)),
                    ).await?;
                }
            } else {
                quote! {}
            };
            create_policy_checks.push(create_policy_check);
            // For create: use the field type directly (required fields stay required)
            create_input_fields.push(quote! {
                #[graphql(name = #graphql_name)]
                pub #field_name: #field_type,
            });

            // Track columns for INSERT
            insert_columns.push(db_col.clone());

            // Generate bind value push based on field type
            // We push to bind_values vector to avoid lifetime issues with ::graphql_orm::sqlx::query
            if meta.is_boolean_field || is_bool_type(field_type) {
                if is_option_type(field_type) {
                    insert_binds.push(quote! {
                        match input.#field_name {
                            Some(b) => bind_values.push(#struct_name::__gom_bool_sql_value(b)),
                            None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                        }
                    });
                } else {
                    insert_binds.push(quote! {
                        bind_values.push(#struct_name::__gom_bool_sql_value(input.#field_name));
                    });
                }
            } else if meta.is_json_field
                || (is_vec_type(field_type) && !is_byte_vec_type(field_type))
            {
                insert_binds.push(quote! {
                    bind_values.push(::graphql_orm::graphql::orm::SqlValue::String(
                        serde_json::to_string(&input.#field_name).unwrap_or_else(|_| "[]".to_string())
                    ));
                });
            } else if is_uuid_type(field_type) {
                if is_option_type(field_type) {
                    insert_binds.push(quote! {
                        match input.#field_name {
                            Some(v) => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Uuid(v)),
                            None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                        }
                    });
                } else {
                    insert_binds.push(quote! {
                        bind_values.push(::graphql_orm::graphql::orm::SqlValue::Uuid(input.#field_name));
                    });
                }
            } else if is_option_type(field_type) {
                let value_expr =
                    maybe_wrap_write_transform(quote! { v.to_string() }, &meta.transform_write);
                if meta.transform_write.is_some() {
                    // Async transform requires .await — use a block
                    insert_binds.push(quote! {
                        match &input.#field_name {
                            Some(v) => {
                                let __transformed = #value_expr;
                                bind_values.push(::graphql_orm::graphql::orm::SqlValue::String(__transformed));
                            }
                            None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                        }
                    });
                } else {
                    insert_binds.push(quote! {
                        match &input.#field_name {
                            Some(v) => bind_values.push(::graphql_orm::graphql::orm::SqlValue::String(#value_expr)),
                            None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                        }
                    });
                }
            } else {
                let value_expr = maybe_wrap_write_transform(
                    quote! { input.#field_name.to_string() },
                    &meta.transform_write,
                );
                if meta.transform_write.is_some() {
                    insert_binds.push(quote! {
                        {
                            let __transformed = #value_expr;
                            bind_values.push(::graphql_orm::graphql::orm::SqlValue::String(__transformed));
                        }
                    });
                } else {
                    insert_binds.push(quote! {
                        bind_values.push(::graphql_orm::graphql::orm::SqlValue::String(#value_expr));
                    });
                }
            }
        }

        // For update: wrap in Option to make all fields optional (skip PK, timestamps, skip_input)
        // But #[input_only] overrides skip_input (allows write-only fields like encrypted credentials)
        let is_timestamp = rust_name == "created_at" || rust_name == "updated_at";
        if !meta.is_primary_key && !is_timestamp && meta.write && (!meta.skip_input || meta.input_only) {
            // All update fields are wrapped in Option (even if already optional)
            // This allows distinguishing between "not provided" and "set to null"
            let update_type = quote! { Option<#field_type> };

            update_input_fields.push(quote! {
                #[graphql(name = #graphql_name)]
                pub #field_name: #update_type,
            });

            // Generate update field check
            let is_already_optional = is_option_type(field_type);
            let update_policy_check = if let Some(policy_key) = &meta.write_policy {
                quote! {
                    if let Some(ref val) = input.#field_name {
                        db.ensure_writable_field(
                            ctx,
                            #entity_name_lit,
                            #graphql_name,
                            Some(#policy_key),
                            current_entity
                                .as_ref()
                                .map(|entity| entity as &(dyn ::std::any::Any + Send + Sync)),
                            Some(val as &(dyn ::std::any::Any + Send + Sync)),
                        ).await?;
                    }
                }
            } else {
                quote! {}
            };
            update_policy_checks.push(update_policy_check);

            if meta.is_boolean_field || is_bool_type(field_type) {
                if is_already_optional {
                    // Option<Option<bool>> case
                    update_field_checks.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            match val {
                                Some(b) => values.push(#struct_name::__gom_bool_sql_value(*b)),
                                None => values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                            }
                        }
                    });
                } else {
                    // Option<bool> case
                    update_field_checks.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            values.push(#struct_name::__gom_bool_sql_value(*val));
                        }
                    });
                }
            } else if meta.is_json_field
                || (is_vec_type(field_type) && !is_byte_vec_type(field_type))
            {
                update_field_checks.push(quote! {
                    if let Some(ref val) = input.#field_name {
                        changed_fields.push(#db_col);
                        set_clauses.push(format!("{} = ?", #db_col));
                        values.push(::graphql_orm::graphql::orm::SqlValue::String(
                            serde_json::to_string(val).unwrap_or_else(|_| "[]".to_string())
                        ));
                    }
                });
            } else if is_uuid_type(field_type) {
                if is_already_optional {
                    update_field_checks.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            match val {
                                Some(v) => values.push(::graphql_orm::graphql::orm::SqlValue::Uuid(*v)),
                                None => values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                            }
                        }
                    });
                } else {
                    update_field_checks.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            values.push(::graphql_orm::graphql::orm::SqlValue::Uuid(*val));
                        }
                    });
                }
            } else if is_already_optional {
                // Field type is already Option<T>, update type is Option<Option<T>>
                let value_expr =
                    maybe_wrap_write_transform(quote! { v.to_string() }, &meta.transform_write);
                if meta.transform_write.is_some() {
                    update_field_checks.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            match val {
                                Some(v) => {
                                    let __transformed = #value_expr;
                                    values.push(::graphql_orm::graphql::orm::SqlValue::String(__transformed));
                                }
                                None => values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                            }
                        }
                    });
                } else {
                    update_field_checks.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            match val {
                                Some(v) => values.push(::graphql_orm::graphql::orm::SqlValue::String(#value_expr)),
                                None => values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                            }
                        }
                    });
                }
            } else {
                // Field type is T, update type is Option<T>
                let value_expr =
                    maybe_wrap_write_transform(quote! { val.to_string() }, &meta.transform_write);
                if meta.transform_write.is_some() {
                    update_field_checks.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            let __transformed = #value_expr;
                            values.push(::graphql_orm::graphql::orm::SqlValue::String(__transformed));
                        }
                    });
                } else {
                    update_field_checks.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            values.push(::graphql_orm::graphql::orm::SqlValue::String(#value_expr));
                        }
                    });
                }
            }
        }
    }

    let has_updated_at_column = fields.iter().any(|f| {
        parse_field_metadata(f)
            .ok()
            .filter(|m| !m.is_relation && !m.skip_db)
            .and_then(|m| {
                f.ident
                    .as_ref()
                    .map(|ident| m.db_column.unwrap_or_else(|| ident.to_string()))
            })
            .is_some_and(|col| col == "updated_at")
    });

    // Build INSERT SQL template
    let insert_placeholders: Vec<&str> = insert_columns.iter().map(|_| "?").collect();
    let insert_sql = if auto_generated_pk {
        let mut columns = vec!["id".to_string()];
        columns.extend(insert_columns.iter().cloned());
        let mut placeholders = vec!["?".to_string()];
        placeholders.extend(insert_placeholders.iter().map(|value| value.to_string()));
        format!(
            "INSERT INTO {} ({}) VALUES ({})",
            table_name,
            columns.join(", "),
            placeholders.join(", ")
        )
    } else {
        format!(
            "INSERT INTO {} ({}) VALUES ({})",
            table_name,
            insert_columns.join(", "),
            insert_placeholders.join(", ")
        )
    };
    let create_mutation_fields = if auto_generated_pk {
        let mut fields = vec!["id".to_string()];
        fields.extend(insert_columns.iter().cloned());
        fields
    } else {
        insert_columns.clone()
    };
    let create_mutation_field_literals: Vec<syn::LitStr> = create_mutation_fields
        .iter()
        .map(|field| syn::LitStr::new(field, struct_name.span()))
        .collect();
    let created_pk_init = if auto_generated_pk {
        if pk_is_uuid {
            quote! {
                let created_pk = ::graphql_orm::uuid::Uuid::new_v4();
            }
        } else {
            quote! {
                let created_pk = ::graphql_orm::uuid::Uuid::new_v4().to_string();
            }
        }
    } else {
        quote! {
            let created_pk = input.#pk_field.clone();
        }
    };
    let prepend_pk_bind = if auto_generated_pk {
        quote! {
            bind_values.push(#created_pk_value);
        }
    } else {
        quote! {}
    };

    // Column list for SQL (unused now but kept for reference)
    let column_names: Vec<String> = fields
        .iter()
        .filter_map(|f| {
            let meta = parse_field_metadata(f).ok()?;
            if meta.is_relation || meta.skip_db {
                return None;
            }
            let name = f.ident.as_ref()?.to_string();
            Some(meta.db_column.unwrap_or(name))
        })
        .collect();
    let _columns_str = column_names.join(", ");

    // Generate additional type names
    let edge_type = syn::Ident::new(&format!("{}Edge", struct_name), struct_name.span());
    let connection_type =
        syn::Ident::new(&format!("{}Connection", struct_name), struct_name.span());
    let edge_type_str = format!("{}Edge", struct_name);
    let connection_type_str = format!("{}Connection", struct_name);
    let create_input_str = format!("Create{}Input", struct_name);
    let update_input_str = format!("Update{}Input", struct_name);
    let result_type_str = format!("{}Result", struct_name);
    let changed_event_str = format!("{}ChangedEvent", struct_name);
    let has_relations = fields.iter().filter_map(|f| parse_field_metadata(f).ok()).any(|m| m.is_relation);
    let relation_preload_list = if has_relations {
        quote! {
            let selection = ctx.field().selection_set().collect::<Vec<_>>();
            if !generic_conn.edges.is_empty() {
                let cursors = generic_conn
                    .edges
                    .iter()
                    .map(|edge| edge.cursor.clone())
                    .collect::<Vec<_>>();
                let mut entities = std::mem::take(&mut generic_conn.edges)
                    .into_iter()
                    .map(|edge| edge.node)
                    .collect::<Vec<_>>();
                <#struct_name as ::graphql_orm::graphql::orm::RelationLoader>::bulk_load_relations(
                    &mut entities,
                    pool,
                    &selection,
                )
                .await
                .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;
                generic_conn.edges = cursors
                    .into_iter()
                    .zip(entities.into_iter())
                    .map(|(cursor, node)| ::graphql_orm::graphql::pagination::Edge { cursor, node })
                    .collect();
            }
        }
    } else {
        quote! {}
    };
    let relation_preload_single = if has_relations {
        quote! {
            if let Some(entity) = entity.as_mut() {
                let selection = ctx.field().selection_set().collect::<Vec<_>>();
                <#struct_name as ::graphql_orm::graphql::orm::RelationLoader>::load_relations(
                    entity,
                    pool,
                    &selection,
                )
                .await
                .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;
            }
        }
    } else {
        quote! {}
    };

    // Generate match arms for searchable fields (used in search_similar)
    let searchable_field_arms: Vec<proc_macro2::TokenStream> = string_filterable_fields
        .iter()
        .map(|(field_name, is_option)| {
            let field_str = field_name.to_string();
            if *is_option {
                quote! {
                    #field_str => entity.#field_name.as_deref(),
                }
            } else {
                quote! {
                    #field_str => Some(entity.#field_name.as_str()),
                }
            }
        })
        .collect();

    let searchable_field_match = if searchable_field_arms.is_empty() {
        quote! { None }
    } else {
        quote! {
            match field {
                #(#searchable_field_arms)*
                _ => None,
            }
        }
    };

    Ok(quote! {
        // ============================================================================
        // Connection/Edge Types (for pagination)
        // ============================================================================

        /// Edge containing a node and cursor
        #[derive(::graphql_orm::async_graphql::SimpleObject, Debug, Clone)]
        #[graphql(name = #edge_type_str)]
        pub struct #edge_type {
            /// The item at the end of the edge
            #[graphql(name = "Node")]
            pub node: #struct_name,
            /// A cursor for pagination
            #[graphql(name = "Cursor")]
            pub cursor: String,
        }

        /// Connection containing edges and page info
        #[derive(::graphql_orm::async_graphql::SimpleObject, Debug, Clone)]
        #[graphql(name = #connection_type_str)]
        pub struct #connection_type {
            /// The edges in this connection
            #[graphql(name = "Edges")]
            pub edges: Vec<#edge_type>,
            /// Pagination information
            #[graphql(name = "PageInfo")]
            pub page_info: ::graphql_orm::graphql::pagination::PageInfo,
        }

        impl #connection_type {
            /// Create from a generic Connection
            pub fn from_generic(conn: ::graphql_orm::graphql::pagination::Connection<#struct_name>) -> Self {
                Self {
                    edges: conn.edges.into_iter().map(|e| #edge_type {
                        node: e.node,
                        cursor: e.cursor,
                    }).collect(),
                    page_info: conn.page_info,
                }
            }

            /// Create an empty connection
            pub fn empty() -> Self {
                Self {
                    edges: Vec::new(),
                    page_info: ::graphql_orm::graphql::pagination::PageInfo::default(),
                }
            }
        }

        // ============================================================================
        // Create/Update Input Types
        // ============================================================================

        /// Input for creating a new #struct_name
        #[derive(::graphql_orm::async_graphql::InputObject, Clone, Debug)]
        #[graphql(name = #create_input_str)]
        pub struct #create_input {
            #(#create_input_fields)*
        }

        /// Input for updating an existing #struct_name
        #[derive(::graphql_orm::async_graphql::InputObject, Clone, Debug, Default)]
        #[graphql(name = #update_input_str)]
        pub struct #update_input {
            #(#update_input_fields)*
        }

        /// Result type for #struct_name mutations
        #[derive(Debug, Clone, ::graphql_orm::async_graphql::SimpleObject)]
        #[graphql(name = #result_type_str)]
        pub struct #result_type {
            #[graphql(name = "Success")]
            pub success: bool,
            #[graphql(name = "Error")]
            pub error: Option<String>,
            #[graphql(name = #struct_name_str)]
            pub entity: Option<#struct_name>,
        }

        impl #result_type {
            /// Create a successful result with the entity
            pub fn ok(entity: #struct_name) -> Self {
                Self { success: true, error: None, entity: Some(entity) }
            }
            /// Create an error result
            pub fn err(msg: impl Into<String>) -> Self {
                Self { success: false, error: Some(msg.into()), entity: None }
            }
        }

        /// Event for #struct_name changes (subscriptions)
        #[derive(Debug, Clone, ::graphql_orm::async_graphql::SimpleObject, serde::Serialize, serde::Deserialize)]
        #[graphql(name = #changed_event_str)]
        pub struct #changed_event {
            #[graphql(name = "Action")]
            pub action: ::graphql_orm::graphql::orm::ChangeAction,
            #[graphql(name = "Id")]
            pub id: #pk_type,
            #[graphql(name = #struct_name_str)]
            pub entity: Option<#struct_name>,
        }

        /// Result of bulk delete by Where filter
        #[derive(Debug, Clone, ::graphql_orm::async_graphql::SimpleObject)]
        #[graphql(name = #delete_many_result_type_str)]
        pub struct #delete_many_result_type {
            pub success: bool,
            pub error: Option<String>,
            #[graphql(name = "DeletedCount")]
            pub deleted_count: i64,
        }

        impl #delete_many_result_type {
            pub fn ok(deleted_count: i64) -> Self {
                Self { success: true, error: None, deleted_count }
            }
            pub fn err(msg: impl Into<String>) -> Self {
                Self { success: false, error: Some(msg.into()), deleted_count: 0 }
            }
        }

        /// Result of bulk update by Where filter
        #[derive(Debug, Clone, ::graphql_orm::async_graphql::SimpleObject)]
        #[graphql(name = #update_many_result_type_str)]
        pub struct #update_many_result_type {
            pub success: bool,
            pub error: Option<String>,
            pub affected_count: i64,
        }

        impl #update_many_result_type {
            pub fn ok(affected_count: i64) -> Self {
                Self { success: true, error: None, affected_count }
            }
            pub fn err(msg: impl Into<String>) -> Self {
                Self { success: false, error: Some(msg.into()), affected_count: 0 }
            }
        }

        // ============================================================================
        // Query Struct
        // ============================================================================

        /// Generated queries for #struct_name
        #[derive(Default)]
        pub struct #queries_struct;

        #[::graphql_orm::async_graphql::Object]
        impl #queries_struct {
            /// Get a list of #plural_name with optional filtering, sorting, and pagination
            #[graphql(name = #list_query_name)]
            async fn list(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = "Where")] where_input: Option<#where_input>,
                #[graphql(name = "OrderBy")] order_by: Option<Vec<#order_by_input>>,
                #[graphql(name = "Page")] page: Option<::graphql_orm::graphql::orm::PageInput>,
            ) -> ::graphql_orm::async_graphql::Result<#connection_type> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, DatabaseFilter, DatabaseOrderBy, EntityQuery, FromSqlRow};
                use ::graphql_orm::graphql::auth::AuthExt;

                let _user = ctx.auth_user()?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database>();
                let pool = db.pool();

                let mut query = EntityQuery::<#struct_name>::new();

                if let Some(ref filter) = where_input {
                    query = query.filter(filter);
                }

                if let Some(ref orders) = order_by {
                    for order in orders {
                        query = query.order_by(order);
                    }
                }

                if query.order_clauses.is_empty() {
                    query = query.default_order();
                }

                if let Some(ref p) = page {
                    query = query.paginate(p);
                }

                let mut generic_conn = query.fetch_connection(pool).await
                    .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;

                #relation_preload_list

                Ok(#connection_type::from_generic(generic_conn))
            }

            /// Get a single #struct_name_str by ID
            #[graphql(name = #single_query_name)]
            async fn get_by_id(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = "Id")] id: #pk_type,
            ) -> ::graphql_orm::async_graphql::Result<Option<#struct_name>> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};
                use ::graphql_orm::graphql::auth::AuthExt;

                let _user = ctx.auth_user()?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database>();
                let pool = db.pool();

                let pk_col = #struct_name::PRIMARY_KEY;
                let mut entity = EntityQuery::<#struct_name>::new()
                    .where_clause(
                        &format!("{} = {}", pk_col, #struct_name::__gom_placeholder(1)),
                        #pk_bind_value
                    )
                    .fetch_one(pool)
                    .await
                    .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;

                #relation_preload_single

                Ok(entity)
            }
        }

        // ============================================================================
        // Mutation Struct
        // ============================================================================

        /// Generated mutations for #struct_name
        #[derive(Default)]
        pub struct #mutations_struct;

        #[::graphql_orm::async_graphql::Object]
        impl #mutations_struct {
            /// Create a new #struct_name_str
            #[graphql(name = #create_mutation_name)]
            async fn create(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = "Input")] input: #create_input,
            ) -> ::graphql_orm::async_graphql::Result<#result_type> {
                use ::graphql_orm::graphql::auth::AuthExt;
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};

                let _user = ctx.auth_user()?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database>();
                let pool = db.pool();

                #created_pk_init
                let sql = #struct_name::__gom_rebind_sql(#insert_sql, 1);

                // Collect all values first
                let mut bind_values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();
                #(#create_policy_checks)*
                #prepend_pk_bind
                #(#insert_binds)*
                let mutation_fields = vec![#(#create_mutation_field_literals),*];
                let mutation_changes = ::graphql_orm::graphql::orm::mutation_changes(&mutation_fields, &bind_values);
                db.run_mutation_hook(
                    ctx,
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Created,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        id: #created_pk_id_string,
                        changes: mutation_changes.clone(),
                    },
                ).await?;

                // Execute using our helper that handles lifetimes properly
                let result = ::graphql_orm::graphql::orm::execute_with_binds(&sql, &bind_values, pool).await;

                match result {
                    Ok(_) => {
                        // Fetch the created entity
                        let entity = EntityQuery::<#struct_name>::new()
                            .where_clause(
                                &format!("{} = {}", #struct_name::PRIMARY_KEY, #struct_name::__gom_placeholder(1)),
                                #created_pk_value
                            )
                            .fetch_one(pool)
                            .await
                            .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?
                            .ok_or_else(|| ::graphql_orm::async_graphql::Error::new("Entity not found after creation"))?;

                        // Broadcast entity change event if subscription channel is configured
                        if let Ok(tx) = ctx.data::<::graphql_orm::tokio::sync::broadcast::Sender<#changed_event>>() {
                            let _ = tx.send(#changed_event {
                                action: ::graphql_orm::graphql::orm::ChangeAction::Created,
                                id: entity.#pk_field.clone(),
                                entity: Some(entity.clone()),
                            });
                        }

                        db.run_mutation_hook(
                            ctx,
                            &::graphql_orm::graphql::orm::MutationEvent {
                                phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                                action: ::graphql_orm::graphql::orm::ChangeAction::Created,
                                entity_name: #entity_name_lit,
                                table_name: #table_name,
                                id: entity.#pk_field.to_string(),
                                changes: mutation_changes.clone(),
                            },
                        ).await?;

                        // Invoke optional post-mutation hook.
                        #notify_on_created

                        Ok(#result_type::ok(entity))
                    }
                    Err(e) => Ok(#result_type::err(e.to_string())),
                }
            }

            /// Update an existing #struct_name_str
            #[graphql(name = #update_mutation_name)]
            async fn update(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = "Id")] id: #pk_type,
                #[graphql(name = "Input")] input: #update_input,
            ) -> ::graphql_orm::async_graphql::Result<#result_type> {
                use ::graphql_orm::graphql::auth::AuthExt;
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};

                let _user = ctx.auth_user()?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database>();
                let pool = db.pool();
                let current_entity = EntityQuery::<#struct_name>::new()
                    .where_clause(
                        &format!("{} = {}", #struct_name::PRIMARY_KEY, #struct_name::__gom_placeholder(1)),
                        #pk_bind_value
                    )
                    .fetch_one(pool)
                    .await
                    .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;

                if current_entity.is_none() {
                    return Ok(#result_type::err("Entity not found"));
                }

                // Build dynamic UPDATE SQL based on provided fields
                let mut set_clauses: Vec<String> = Vec::new();
                let mut changed_fields: Vec<&str> = Vec::new();
                let mut values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();

                #(#update_policy_checks)*
                #(#update_field_checks)*

                // Update timestamp column when this entity defines one
                if #has_updated_at_column {
                    set_clauses.push(format!("updated_at = {}", #struct_name::__gom_current_epoch_expr()));
                }

                if set_clauses.is_empty() {
                    return Ok(#result_type::err("No fields to update"));
                }
                let mutation_changes = ::graphql_orm::graphql::orm::mutation_changes(&changed_fields, &values);
                db.run_mutation_hook(
                    ctx,
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        id: id.to_string(),
                        changes: mutation_changes.clone(),
                    },
                ).await?;

                let sql = #struct_name::__gom_rebind_sql(&format!(
                    "UPDATE {} SET {} WHERE {} = ?",
                    #table_name,
                    set_clauses.join(", "),
                    #struct_name::PRIMARY_KEY
                ), 1);

                // Add the ID to the values for the WHERE clause
                values.push(#pk_bind_value);

                let result = ::graphql_orm::graphql::orm::execute_with_binds(&sql, &values, pool).await;

                match result {
                    Ok(r) if r.rows_affected() > 0 => {
                        // Fetch the updated entity
                        let entity = EntityQuery::<#struct_name>::new()
                            .where_clause(
                                &format!("{} = {}", #struct_name::PRIMARY_KEY, #struct_name::__gom_placeholder(1)),
                                #pk_bind_value
                            )
                            .fetch_one(pool)
                            .await
                            .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;

                        match entity {
                            Some(entity) => {
                                // Broadcast entity change event if subscription channel is configured
                                if let Ok(tx) = ctx.data::<::graphql_orm::tokio::sync::broadcast::Sender<#changed_event>>() {
                                    let _ = tx.send(#changed_event {
                                        action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                                        id: entity.#pk_field.clone(),
                                        entity: Some(entity.clone()),
                                    });
                                }

                                db.run_mutation_hook(
                                    ctx,
                                    &::graphql_orm::graphql::orm::MutationEvent {
                                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                                        action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                                        entity_name: #entity_name_lit,
                                        table_name: #table_name,
                                        id: entity.#pk_field.to_string(),
                                        changes: mutation_changes.clone(),
                                    },
                                ).await?;

                                // Invoke optional post-mutation hook.
                                #notify_on_updated

                                Ok(#result_type::ok(entity))
                            },
                            None => Ok(#result_type::err("Entity not found after update")),
                        }
                    }
                    Ok(_) => Ok(#result_type::err("Entity not found")),
                    Err(e) => Ok(#result_type::err(e.to_string())),
                }
            }

            /// Delete a #struct_name_str
            #[graphql(name = #delete_mutation_name)]
            async fn delete(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = "Id")] id: #pk_type,
            ) -> ::graphql_orm::async_graphql::Result<#result_type> {
                use ::graphql_orm::graphql::auth::AuthExt;
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, SqlValue};

                let _user = ctx.auth_user()?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database>();
                let pool = db.pool();

                // Fetch entity before deletion for notification purposes
                let entity = EntityQuery::<#struct_name>::new()
                    .where_clause(
                        &format!("{} = {}", #struct_name::PRIMARY_KEY, #struct_name::__gom_placeholder(1)),
                        #pk_bind_value
                    )
                    .fetch_one(pool)
                    .await
                    .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;

                if entity.is_none() {
                    return Ok(#result_type::err("Entity not found"));
                }
                let entity = entity.unwrap();
                db.run_mutation_hook(
                    ctx,
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        id: entity.#pk_field.to_string(),
                        changes: Vec::new(),
                    },
                ).await?;

                let sql = #struct_name::__gom_rebind_sql(
                    &format!("DELETE FROM {} WHERE {} = ?", #table_name, #struct_name::PRIMARY_KEY),
                    1
                );
                let values = vec![#pk_bind_value];
                let result = ::graphql_orm::graphql::orm::execute_with_binds(&sql, &values, pool).await;

                match result {
                    Ok(r) if r.rows_affected() > 0 => {
                        // Broadcast entity change event if subscription channel is configured
                        if let Ok(tx) = ctx.data::<::graphql_orm::tokio::sync::broadcast::Sender<#changed_event>>() {
                            let _ = tx.send(#changed_event {
                                action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                                id: entity.#pk_field.clone(),
                                entity: Some(entity.clone()),
                            });
                        }

                        db.run_mutation_hook(
                            ctx,
                            &::graphql_orm::graphql::orm::MutationEvent {
                                phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                                action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                                entity_name: #entity_name_lit,
                                table_name: #table_name,
                                id: entity.#pk_field.to_string(),
                                changes: Vec::new(),
                            },
                        ).await?;

                        // Invoke optional post-mutation hook.
                        #notify_on_deleted

                        Ok(#result_type {
                            success: true,
                            error: None,
                            entity: None
                        })
                    },
                    Ok(_) => Ok(#result_type::err("Entity not found")),
                    Err(e) => Ok(#result_type::err(e.to_string())),
                }
            }

            /// Update multiple #plural_name matching the given Where filter
            #[graphql(name = #update_many_mutation_name)]
            async fn update_many(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = "Where")] where_input: Option<#where_input>,
                #[graphql(name = "Input")] input: #update_input,
            ) -> ::graphql_orm::async_graphql::Result<#update_many_result_type> {
                use ::graphql_orm::graphql::auth::AuthExt;
                use ::graphql_orm::graphql::orm::{DatabaseFilter, EntityQuery};

                let _user = ctx.auth_user()?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database>();
                let pool = db.pool();

                let filter = match where_input {
                    Some(ref f) if !f.is_empty() => f,
                    _ => return Ok(#update_many_result_type::err("Where filter is required for bulk update and must not be empty")),
                };

                // Build dynamic UPDATE SQL based on provided fields
                let mut set_clauses: Vec<String> = Vec::new();
                let mut changed_fields: Vec<&str> = Vec::new();
                let mut values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();

                #(#update_field_checks)*

                // Update timestamp column when this entity defines one
                if #has_updated_at_column {
                    set_clauses.push(format!("updated_at = {}", #struct_name::__gom_current_epoch_expr()));
                }

                if set_clauses.is_empty() {
                    return Ok(#update_many_result_type::err("No fields to update"));
                }

                // Reuse EntityQuery WHERE SQL construction and bind values.
                let query = EntityQuery::<#struct_name>::new().filter(filter);
                let (delete_sql, filter_values) = query.build_delete_sql();
                let where_clause = match delete_sql.split_once(" WHERE ") {
                    Some((_, clause)) => #struct_name::__gom_rebind_sql(clause, values.len() + 1),
                    None => return Ok(#update_many_result_type::err("Where filter produced empty SQL")),
                };

                let sql = format!(
                    "UPDATE {} SET {} WHERE {}",
                    #table_name,
                    set_clauses.join(", "),
                    where_clause
                );

                values.extend(filter_values);
                let result = ::graphql_orm::graphql::orm::execute_with_binds(&sql, &values, pool).await;

                match result {
                    Ok(r) => Ok(#update_many_result_type::ok(r.rows_affected() as i64)),
                    Err(e) => Ok(#update_many_result_type::err(e.to_string())),
                }
            }

            /// Delete multiple #plural_name matching the given Where filter
            #[graphql(name = #delete_many_mutation_name)]
            async fn delete_many(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = "Where")] where_input: Option<#where_input>,
            ) -> ::graphql_orm::async_graphql::Result<#delete_many_result_type> {
                use ::graphql_orm::graphql::auth::AuthExt;
                use ::graphql_orm::graphql::orm::{DatabaseEntity, DatabaseFilter, EntityQuery, FromSqlRow};

                let _user = ctx.auth_user()?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database>();
                let pool = db.pool();

                let filter = match where_input {
                    Some(ref f) if !f.is_empty() => f,
                    _ => return Ok(#delete_many_result_type::err("Where filter is required for bulk delete and must not be empty")),
                };

                let mut query = EntityQuery::<#struct_name>::new().filter(filter);
                let (sql, values) = query.build_delete_sql();
                let sql = #struct_name::__gom_rebind_sql(&sql, 1);

                let result = ::graphql_orm::graphql::orm::execute_with_binds(&sql, &values, pool).await;

                match result {
                    Ok(r) => Ok(#delete_many_result_type::ok(r.rows_affected() as i64)),
                    Err(e) => Ok(#delete_many_result_type::err(e.to_string())),
                }
            }
        }

        // ============================================================================
        // Subscription Struct
        // ============================================================================

        /// Generated subscriptions for #struct_name
        #[derive(Default)]
        pub struct #subscriptions_struct;

        #[::graphql_orm::async_graphql::Subscription]
        impl #subscriptions_struct {
            /// Subscribe to #struct_name_str changes
            #[graphql(name = #subscription_name)]
            async fn on_changed(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = "Filter")] _filter: Option<::graphql_orm::graphql::orm::SubscriptionFilterInput>,
            ) -> ::graphql_orm::async_graphql::Result<impl ::graphql_orm::futures::Stream<Item = #changed_event>> {
                use ::graphql_orm::futures::stream::{self, StreamExt};
                use ::graphql_orm::graphql::auth::AuthExt;

                let _user = ctx.auth_user()?;

                // Try to get the broadcast channel for this entity type
                // If not available, return an empty stream (subscription not enabled)
                let maybe_events = ctx.data_opt::<::graphql_orm::tokio::sync::broadcast::Sender<#changed_event>>();

                Ok(match maybe_events {
                    None => {
                        // Return empty stream if no broadcast channel is configured
                        stream::empty().left_stream()
                    }
                    Some(events) => {
                        let rx = events.subscribe();

                        use ::graphql_orm::tokio_stream::wrappers::BroadcastStream;

                        BroadcastStream::new(rx)
                            .filter_map(move |result| async move {
                                match result {
                                    Ok(event) => Some(event),
                                    Err(_) => None,
                                }
                            })
                            .right_stream()
                    }
                })
            }
        }

        // ============================================================================
        // Repository Trait Implementation
        // ============================================================================

        /// Repository implementation for #struct_name
        ///
        /// Provides static async methods for common database operations.
        impl #struct_name {
            /// Insert a new entity record using the generated create input.
            pub async fn insert(
                pool: &#pool_type,
                input: #create_input,
            ) -> Result<Self, ::graphql_orm::sqlx::Error> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};

                #created_pk_init
                let sql = Self::__gom_rebind_sql(#insert_sql, 1);

                let mut bind_values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();
                #prepend_pk_bind
                #(#insert_binds)*

                ::graphql_orm::graphql::orm::execute_with_binds(&sql, &bind_values, pool).await?;

                EntityQuery::<Self>::new()
                    .where_clause(
                        &format!("{} = {}", <Self as DatabaseEntity>::PRIMARY_KEY, Self::__gom_placeholder(1)),
                        #created_pk_value,
                    )
                    .fetch_one(pool)
                    .await?
                    .ok_or(::graphql_orm::sqlx::Error::RowNotFound)
            }

            /// Find all entities matching the given filter
            pub fn query<'a>(pool: &'a #pool_type) -> ::graphql_orm::graphql::orm::FindQuery<'a, Self, #where_input, #order_by_input> {
                ::graphql_orm::graphql::orm::FindQuery::new(pool)
            }

            /// Find entity by ID
            pub async fn get(pool: &#pool_type, id: &#pk_type_ty) -> Result<Option<Self>, ::graphql_orm::sqlx::Error> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};

                EntityQuery::<Self>::new()
                    .where_clause(
                        &format!("{} = {}", <Self as DatabaseEntity>::PRIMARY_KEY, Self::__gom_placeholder(1)),
                        #pk_bind_value_ref
                    )
                    .fetch_one(pool)
                    .await
            }

            /// Count entities matching the given filter
            pub fn count_query<'a>(pool: &'a #pool_type) -> ::graphql_orm::graphql::orm::CountQuery<'a, #where_input> {
                use ::graphql_orm::graphql::orm::DatabaseEntity;
                ::graphql_orm::graphql::orm::CountQuery::new(pool, <Self as DatabaseEntity>::TABLE_NAME)
            }

            /// Search entities with fuzzy/similar text matching
            ///
            /// # Arguments
            /// * `pool` - Database connection pool
            /// * `field` - Name of the field to search (snake_case)
            /// * `query` - The search query text
            /// * `threshold` - Minimum similarity score (0.0-1.0, recommended: 0.5-0.7)
            /// * `filter` - Optional additional filter to apply
            /// * `limit` - Maximum number of results to return
            ///
            /// # Returns
            /// Vector of (entity, score) tuples, sorted by score descending
            ///
            /// # Example
            /// ```rust,ignore
            /// let matches = Entity::search_similar(
            ///     &pool,
            ///     "name",
            ///     "example",
            ///     0.6,
            ///     Some(EntityWhereInput {
            ///         name: Some(StringFilter::contains("ex")),
            ///         ..Default::default()
            ///     }),
            ///     Some(25),
            /// ).await?;
            ///
            /// for (entity, score) in matches {
            ///     println!("{}: {:.2}", entity.name, score);
            /// }
            /// ```
            pub async fn search_similar(
                pool: &#pool_type,
                field: &str,
                query: &str,
                threshold: f64,
                filter: Option<#where_input>,
                limit: Option<i64>,
            ) -> Result<Vec<(Self, f64)>, ::graphql_orm::sqlx::Error> {
                use ::graphql_orm::graphql::orm::FuzzyMatcher;

                // Fetch candidates (optionally filtered)
                let mut q = Self::query(pool);
                if let Some(f) = filter {
                    q = q.filter(f);
                }
                // Fetch more than limit to account for fuzzy filtering
                if let Some(l) = limit {
                    q = q.limit(l * 5);
                }
                let candidates = q.fetch_all().await?;

                // Score with fuzzy matcher
                let matcher = FuzzyMatcher::new(query).with_threshold(threshold);
                let mut results = matcher.filter_and_score(candidates, |entity| {
                    Self::get_searchable_field(entity, field)
                });

                // Apply limit
                if let Some(l) = limit {
                    results.truncate(l as usize);
                }

                Ok(results.into_iter().map(|m| (m.entity, m.score)).collect())
            }

            /// Get a searchable field value by name (for fuzzy matching)
            #[doc(hidden)]
            fn get_searchable_field<'a>(entity: &'a Self, field: &str) -> Option<&'a str> {
                #searchable_field_match
            }
        }
    })
}

// ============================================================================
// schema_roots! procedural macro
// ============================================================================

/// Build QueryRoot and MutationRoot from entity names and optional custom-ops list.
///
/// Generated types per entity: `XQueries`, `XMutations`, and optionally `XCustomOperations`.
/// The single source of truth is the entity list; no need to list each *Queries/*Mutations type.
///
/// # Usage
///
/// ```ignore
/// macros::schema_roots! {
///     query_custom_ops: [Entity, RelatedEntity, AuditLog],
///     entities: [
///         Entity, RelatedEntity, AuditLog, User, Role, Permission,
///         ApiKey, Session, Job, JobRun, Attachment, Tag,
///     ],
/// }
/// ```
///
/// Expands to:
/// - `#[derive(MergedObject, Default)] pub struct QueryRoot(EntityCustomOperations, ..., EntityQueries, ...);`
/// - `#[derive(MergedObject, Default)] pub struct MutationRoot(EntityMutations, ...);`
/// - `#[derive(MergedSubscription, Default)] pub struct SubscriptionRoot(EntitySubscriptions, ...);`
#[proc_macro]
pub fn schema_roots(input: TokenStream) -> TokenStream {
    let args = match syn::parse::<SchemaRootsArgs>(input) {
        Ok(a) => a,
        Err(e) => return e.to_compile_error().into(),
    };

    let query_custom_ops = &args.query_custom_ops;
    let entities = &args.entities;

    // Use mixed_site so generated idents don't trigger "proc-macro map is missing error entry for crate"
    let span = proc_macro2::Span::mixed_site();
    // Types for QueryRoot: XCustomOperations for each in query_custom_ops, then extra_query_types, then XQueries for each entity
    let custom_op_types: Vec<proc_macro2::TokenStream> = query_custom_ops
        .iter()
        .map(|e| {
            let name = syn::Ident::new(&format!("{}CustomOperations", e), span);
            quote! { #name }
        })
        .chain(args.extra_query_types.iter().map(|e| quote! { #e }))
        .collect();
    let query_types: Vec<proc_macro2::TokenStream> = entities
        .iter()
        .map(|e| {
            let name = syn::Ident::new(&format!("{}Queries", e), span);
            quote! { #name }
        })
        .collect();

    // Types for MutationRoot: extra_mutation_types first, then XMutations for each entity
    let extra_mutation_type_streams: Vec<proc_macro2::TokenStream> = args
        .extra_mutation_types
        .iter()
        .map(|e| quote! { #e })
        .collect();
    let mutation_custom_ops = if extra_mutation_type_streams.is_empty() {
        None
    } else {
        Some(extra_mutation_type_streams.as_slice())
    };
    let mutation_types: Vec<proc_macro2::TokenStream> = entities
        .iter()
        .map(|e| {
            let name = syn::Ident::new(&format!("{}Mutations", e), span);
            quote! { #name }
        })
        .collect();

    // Types for SubscriptionRoot: extra_subscription_types first, then XSubscriptions for each entity
    let extra_subscription_type_streams: Vec<proc_macro2::TokenStream> = args
        .extra_subscription_types
        .iter()
        .map(|e| quote! { #e })
        .collect();
    let subscription_custom_ops = if extra_subscription_type_streams.is_empty() {
        None
    } else {
        Some(extra_subscription_type_streams.as_slice())
    };
    let subscription_types: Vec<proc_macro2::TokenStream> = entities
        .iter()
        .map(|e| {
            let name = syn::Ident::new(&format!("{}Subscriptions", e), span);
            quote! { #name }
        })
        .collect();

    // Chunk types to avoid exceeding async-graphql's MergedObject recursion limit (~15–20 deep).
    let query_custom_chunk = if custom_op_types.is_empty() {
        None
    } else {
        Some(custom_op_types.as_slice())
    };
    let query_root = emit_chunked_merged(
        "Query",
        query_custom_chunk,
        &query_types,
        async_graphql_merged_object_derive(),
    );
    let mutation_root = emit_chunked_merged(
        "Mutation",
        mutation_custom_ops,
        &mutation_types,
        async_graphql_merged_object_derive(),
    );
    let subscription_root = emit_chunked_merged_subscription(
        "Subscription",
        subscription_custom_ops,
        &subscription_types,
    );
    let schema_loader_data: Vec<proc_macro2::TokenStream> = entities
        .iter()
        .map(|entity| {
            quote! {
                let builder = builder.data(
                    ::graphql_orm::async_graphql::dataloader::DataLoader::new(
                        ::graphql_orm::graphql::loaders::RelationLoader::<#entity>::new(database.clone()),
                        ::graphql_orm::tokio::spawn,
                    )
                );
            }
        })
        .collect();

    quote! {
        #query_root
        #mutation_root
        #subscription_root

        pub type AppSchema = ::graphql_orm::async_graphql::Schema<QueryRoot, MutationRoot, SubscriptionRoot>;

        pub fn schema_builder(
            database: ::graphql_orm::db::Database,
        ) -> ::graphql_orm::async_graphql::SchemaBuilder<QueryRoot, MutationRoot, SubscriptionRoot> {
            let builder = ::graphql_orm::async_graphql::Schema::build(
                QueryRoot::default(),
                MutationRoot::default(),
                SubscriptionRoot::default(),
            )
            .data(database.clone());
            #(#schema_loader_data)*
            builder
        }
    }
    .into()
}

fn async_graphql_merged_object_derive() -> proc_macro2::TokenStream {
    quote! { ::graphql_orm::async_graphql::MergedObject }
}

fn emit_chunked_merged(
    name: &str,
    custom_ops: Option<&[proc_macro2::TokenStream]>,
    types: &[proc_macro2::TokenStream],
    derive_macro: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    let chunk_size = 12usize;
    let mut chunk_defs = Vec::new();
    let mut root_chunk_idents = Vec::new();

    // Custom ops chunk (Query only)
    if let Some(ops) = custom_ops {
        if !ops.is_empty() {
            let chunk_name = syn::Ident::new(
                &format!("{}RootCustomOpsChunk", name),
                proc_macro2::Span::mixed_site(),
            );
            let def = quote! {
                #[derive(#derive_macro, Default)]
                pub struct #chunk_name(
                    #(#ops),*
                );
            };
            chunk_defs.push(def);
            root_chunk_idents.push(chunk_name);
        }
    }

    // Entity type chunks
    for (i, chunk_types) in types.chunks(chunk_size).enumerate() {
        let chunk_name = syn::Ident::new(
            &format!("{}RootChunk{}", name, i),
            proc_macro2::Span::mixed_site(),
        );
        let def = quote! {
            #[derive(#derive_macro, Default)]
            pub struct #chunk_name(
                #(#chunk_types),*
            );
        };
        chunk_defs.push(def);
        root_chunk_idents.push(chunk_name);
    }

    let root_name = syn::Ident::new(&format!("{}Root", name), proc_macro2::Span::mixed_site());
    let root_def = quote! {
        #[derive(#derive_macro, Default)]
        pub struct #root_name(
            #(#root_chunk_idents),*
        );
    };

    quote! {
        #(#chunk_defs)*
        #root_def
    }
}

fn emit_chunked_merged_subscription(
    name: &str,
    custom_ops: Option<&[proc_macro2::TokenStream]>,
    types: &[proc_macro2::TokenStream],
) -> proc_macro2::TokenStream {
    let derive_macro = quote! { ::graphql_orm::async_graphql::MergedSubscription };
    emit_chunked_merged(name, custom_ops, types, derive_macro)
}

struct SchemaRootsArgs {
    query_custom_ops: Vec<Ident>,
    entities: Vec<Ident>,
    extra_mutation_types: Vec<Ident>,
    extra_query_types: Vec<Ident>,
    extra_subscription_types: Vec<Ident>,
}

impl Parse for SchemaRootsArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        fn parse_list(content: ParseStream) -> syn::Result<Vec<Ident>> {
            let list = Punctuated::<Ident, Token![,]>::parse_terminated(content)?;
            Ok(list.into_iter().collect())
        }

        // query_custom_ops: [ ... ],
        let label: Ident = input.parse()?;
        if label != "query_custom_ops" {
            return Err(syn::Error::new(label.span(), "expected `query_custom_ops`"));
        }
        input.parse::<Token![:]>()?;
        let content;
        syn::bracketed!(content in input);
        let query_custom_ops = parse_list(&content)?;
        let _: Option<Token![,]> = input.parse().ok();

        // entities: [ ... ]
        let label: Ident = input.parse()?;
        if label != "entities" {
            return Err(syn::Error::new(label.span(), "expected `entities`"));
        }
        input.parse::<Token![:]>()?;
        let content;
        syn::bracketed!(content in input);
        let entities = parse_list(&content)?;
        let _: Option<Token![,]> = input.parse().ok();

        // optional extra_mutation_types, extra_query_types, extra_subscription_types
        let mut extra_mutation_types = Vec::new();
        let mut extra_query_types = Vec::new();
        let mut extra_subscription_types = Vec::new();
        while input.peek(Ident) {
            let label: Ident = input.parse()?;
            if label == "extra_mutation_types" {
                input.parse::<Token![:]>()?;
                let content;
                syn::bracketed!(content in input);
                extra_mutation_types = parse_list(&content)?;
                let _: Option<Token![,]> = input.parse().ok();
            } else if label == "extra_query_types" {
                input.parse::<Token![:]>()?;
                let content;
                syn::bracketed!(content in input);
                extra_query_types = parse_list(&content)?;
                let _: Option<Token![,]> = input.parse().ok();
            } else if label == "extra_subscription_types" {
                input.parse::<Token![:]>()?;
                let content;
                syn::bracketed!(content in input);
                extra_subscription_types = parse_list(&content)?;
                let _: Option<Token![,]> = input.parse().ok();
            } else {
                return Err(syn::Error::new(
                    label.span(),
                    "expected `extra_mutation_types`, `extra_query_types`, or `extra_subscription_types`",
                ));
            }
        }

        Ok(SchemaRootsArgs {
            query_custom_ops,
            entities,
            extra_mutation_types,
            extra_query_types,
            extra_subscription_types,
        })
    }
}
