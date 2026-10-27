use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BackendKind {
    Sqlite,
    Postgres,
    Mssql,
}

impl BackendKind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::Postgres => "postgres",
            Self::Mssql => "mssql",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "sqlite" => Some(Self::Sqlite),
            "postgres" | "postgresql" => Some(Self::Postgres),
            "mssql" | "sqlserver" | "sql-server" => Some(Self::Mssql),
            _ => None,
        }
    }

    fn feature_enabled(self) -> bool {
        match self {
            Self::Sqlite => cfg!(feature = "sqlite"),
            Self::Postgres => cfg!(feature = "postgres"),
            Self::Mssql => cfg!(feature = "mssql"),
        }
    }
}

pub(crate) fn enabled_backends() -> Vec<BackendKind> {
    let mut backends = Vec::new();
    if cfg!(feature = "sqlite") {
        backends.push(BackendKind::Sqlite);
    }
    if cfg!(feature = "postgres") {
        backends.push(BackendKind::Postgres);
    }
    if cfg!(feature = "mssql") {
        backends.push(BackendKind::Mssql);
    }
    backends
}

pub(crate) fn resolve_backend(
    requested: Option<&str>,
    span: proc_macro2::Span,
    context: &str,
) -> syn::Result<BackendKind> {
    if let Some(requested) = requested {
        let backend = BackendKind::from_name(requested).ok_or_else(|| {
            syn::Error::new(
                span,
                format!(
                    "unsupported graphql-orm backend `{requested}`; expected sqlite, postgres, or mssql"
                ),
            )
        })?;
        if !backend.feature_enabled() {
            return Err(syn::Error::new(
                span,
                format!(
                    "graphql-orm backend `{}` was requested for {context}, but the `{}` feature is not enabled",
                    backend.name(),
                    backend.name()
                ),
            ));
        }
        return Ok(backend);
    }

    let enabled = enabled_backends();
    match enabled.as_slice() {
        [backend] => Ok(*backend),
        [] => Err(syn::Error::new(
            span,
            "enable at least one graphql-orm backend feature: sqlite, postgres, or mssql",
        )),
        _ => Err(syn::Error::new(
            span,
            format!(
                "multiple graphql-orm backend features are enabled ({}); specify backend = \"sqlite\", \"postgres\", or \"mssql\" on {context}",
                enabled
                    .iter()
                    .map(|backend| backend.name())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

pub(crate) fn backend_marker_tokens(backend: BackendKind) -> proc_macro2::TokenStream {
    match backend {
        BackendKind::Sqlite => quote! { ::graphql_orm::SqliteBackend },
        BackendKind::Postgres => quote! { ::graphql_orm::PostgresBackend },
        BackendKind::Mssql => quote! { ::graphql_orm::MssqlBackend },
    }
}

pub(crate) fn backend_row_type_tokens(backend: BackendKind) -> proc_macro2::TokenStream {
    let marker = backend_marker_tokens(backend);
    quote! { <#marker as ::graphql_orm::OrmBackend>::Row }
}

pub(crate) fn backend_pool_type_tokens(backend: BackendKind) -> proc_macro2::TokenStream {
    let marker = backend_marker_tokens(backend);
    quote! { <#marker as ::graphql_orm::OrmBackend>::Pool }
}

pub(crate) fn backend_dialect_expr(backend: BackendKind) -> proc_macro2::TokenStream {
    let marker = backend_marker_tokens(backend);
    quote! { <#marker as ::graphql_orm::OrmBackend>::DIALECT }
}

pub(crate) fn backend_helper_import_tokens(backend: BackendKind) -> proc_macro2::TokenStream {
    match backend {
        BackendKind::Sqlite => quote! {
            use ::graphql_orm::sqlx::Row;
            use ::graphql_orm::db::sqlite_helpers::*;
        },
        BackendKind::Postgres => quote! {
            use ::graphql_orm::sqlx::Row;
            use ::graphql_orm::db::postgres_helpers::*;
        },
        BackendKind::Mssql => quote! {
            use ::graphql_orm::db::mssql_helpers::*;
        },
    }
}

pub(crate) fn backend_current_epoch_expr(backend: BackendKind) -> &'static str {
    // Store defaults without redundant outer parentheses. SQLite DDL rendering
    // re-wraps non-literal defaults as `DEFAULT (expr)`, and PRAGMA table_info
    // returns the unwrapped form (`unixepoch()`). Keeping metadata unwrapped
    // avoids false AlterColumn plans after reopening a file-backed database.
    match backend {
        BackendKind::Postgres => "EXTRACT(EPOCH FROM NOW())::bigint",
        BackendKind::Mssql => "DATEDIFF_BIG(second, '1970-01-01', SYSUTCDATETIME())",
        BackendKind::Sqlite => "unixepoch()",
    }
}

pub(crate) fn backend_quote_identifier(backend: BackendKind, identifier: &str) -> String {
    if backend == BackendKind::Mssql {
        if (identifier.starts_with('[') && identifier.ends_with(']'))
            || (identifier.starts_with('"') && identifier.ends_with('"'))
        {
            identifier.to_string()
        } else {
            format!("[{}]", identifier.replace(']', "]]"))
        }
    } else {
        identifier.to_string()
    }
}

// Keep these in sync with `graphql_orm::graphql::orm::dialect`; the runtime
// crate's `macro_reserved_identifier_lists_match_the_runtime` test compares them.
const SQLITE_RESERVED_IDENTIFIERS: &[&str] = &[
    "add",
    "all",
    "alter",
    "and",
    "as",
    "autoincrement",
    "between",
    "case",
    "cast",
    "check",
    "collate",
    "commit",
    "constraint",
    "create",
    "current_date",
    "current_time",
    "current_timestamp",
    "default",
    "deferrable",
    "delete",
    "distinct",
    "drop",
    "else",
    "escape",
    "except",
    "exists",
    "foreign",
    "from",
    "group",
    "having",
    "if",
    "in",
    "index",
    "insert",
    "intersect",
    "into",
    "is",
    "isnull",
    "join",
    "limit",
    "not",
    "nothing",
    "notnull",
    "null",
    "on",
    "or",
    "order",
    "primary",
    "raise",
    "references",
    "returning",
    "select",
    "set",
    "table",
    "then",
    "to",
    "transaction",
    "union",
    "unique",
    "update",
    "using",
    "values",
    "when",
    "where",
];

const POSTGRES_RESERVED_IDENTIFIERS: &[&str] = &[
    "all",
    "analyse",
    "analyze",
    "and",
    "any",
    "array",
    "as",
    "asc",
    "asymmetric",
    "authorization",
    "binary",
    "both",
    "case",
    "cast",
    "check",
    "collate",
    "collation",
    "column",
    "concurrently",
    "constraint",
    "create",
    "cross",
    "current_catalog",
    "current_date",
    "current_role",
    "current_schema",
    "current_time",
    "current_timestamp",
    "current_user",
    "default",
    "deferrable",
    "desc",
    "distinct",
    "do",
    "else",
    "end",
    "except",
    "false",
    "fetch",
    "for",
    "foreign",
    "freeze",
    "from",
    "full",
    "grant",
    "group",
    "having",
    "ilike",
    "in",
    "initially",
    "inner",
    "intersect",
    "into",
    "is",
    "isnull",
    "join",
    "lateral",
    "leading",
    "left",
    "like",
    "limit",
    "localtime",
    "localtimestamp",
    "natural",
    "not",
    "notnull",
    "null",
    "offset",
    "on",
    "only",
    "or",
    "order",
    "outer",
    "overlaps",
    "placing",
    "primary",
    "references",
    "returning",
    "right",
    "select",
    "session_user",
    "similar",
    "some",
    "symmetric",
    "system_user",
    "table",
    "tablesample",
    "then",
    "to",
    "trailing",
    "true",
    "union",
    "unique",
    "user",
    "using",
    "variadic",
    "verbose",
    "when",
    "where",
    "window",
    "with",
];

// SQL identifier delimiters may contain dots and doubled closing delimiters.
// Slice only at unquoted dots; byte offsets are safe because delimiters are ASCII.
fn identifier_path_parts(identifier: &str) -> impl Iterator<Item = &str> {
    let bytes = identifier.as_bytes();
    let mut start = 0;
    let mut offset = 0;
    std::iter::from_fn(move || {
        let mut closing = None;
        while offset < bytes.len() {
            let character = bytes[offset];
            offset += 1;
            match closing {
                Some(delimiter) if character == delimiter => {
                    if bytes.get(offset) == Some(&delimiter) {
                        offset += 1;
                    } else {
                        closing = None;
                    }
                }
                None => match character {
                    b'"' | b'`' if offset - 1 == start => closing = Some(character),
                    b'[' if offset - 1 == start => closing = Some(b']'),
                    b'.' => {
                        let part = &identifier[start..offset - 1];
                        start = offset;
                        return Some(part);
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        if start <= bytes.len() {
            let part = &identifier[start..];
            start = bytes.len() + 1;
            Some(part)
        } else {
            None
        }
    })
}

/// An identifier as generated DML writes it, matching the runtime
/// `DatabaseBackend::render_identifier`: SQLite and PostgreSQL leave plain,
/// non-reserved names bare and delimit everything else; MSSQL brackets every
/// name as [`backend_quote_identifier`] does.
pub(crate) fn backend_render_identifier(backend: BackendKind, identifier: &str) -> String {
    let reserved = match backend {
        BackendKind::Mssql => return backend_quote_identifier(backend, identifier),
        BackendKind::Sqlite => SQLITE_RESERVED_IDENTIFIERS,
        BackendKind::Postgres => POSTGRES_RESERVED_IDENTIFIERS,
    };
    if identifier.len() >= 2
        && ((identifier.starts_with('"') && identifier.ends_with('"'))
            || (backend == BackendKind::Sqlite
                && ((identifier.starts_with('[') && identifier.ends_with(']'))
                    || (identifier.starts_with('`') && identifier.ends_with('`')))))
    {
        return identifier.to_string();
    }
    let mut characters = identifier.chars();
    let plain_start = |character: char| {
        character.is_ascii_alphabetic() || character == '_' || !character.is_ascii()
    };
    let plain = characters.next().is_some_and(plain_start)
        && characters.all(|character| {
            plain_start(character) || character.is_ascii_digit() || character == '$'
        });
    if plain
        && reserved
            .binary_search(&identifier.to_ascii_lowercase().as_str())
            .is_err()
    {
        identifier.to_string()
    } else {
        format!("\"{}\"", identifier.replace('"', "\"\""))
    }
}

/// [`backend_render_identifier`] for each part, preserving quoted dots and escapes.
pub(crate) fn backend_render_identifier_path(backend: BackendKind, identifier: &str) -> String {
    identifier_path_parts(identifier)
        .filter(|part| !part.is_empty())
        .map(|part| backend_render_identifier(backend, part))
        .collect::<Vec<_>>()
        .join(".")
}

/// The runtime `DatabaseBackend` value for generated code that renders a
/// name only known at runtime, such as another entity's table.
pub(crate) fn backend_dialect_tokens(backend: BackendKind) -> proc_macro2::TokenStream {
    match backend {
        BackendKind::Sqlite => quote! { ::graphql_orm::graphql::orm::DatabaseBackend::Sqlite },
        BackendKind::Postgres => quote! { ::graphql_orm::graphql::orm::DatabaseBackend::Postgres },
        BackendKind::Mssql => quote! { ::graphql_orm::graphql::orm::DatabaseBackend::Mssql },
    }
}

pub(crate) fn backend_quote_identifier_path(backend: BackendKind, identifier: &str) -> String {
    if backend == BackendKind::Mssql {
        identifier_path_parts(identifier)
            .filter(|part| !part.is_empty())
            .map(|part| backend_quote_identifier(backend, part))
            .collect::<Vec<_>>()
            .join(".")
    } else {
        identifier.to_string()
    }
}

#[cfg(test)]
mod identifier_path_tests {
    use super::*;

    #[test]
    fn quoted_identifier_paths_match_the_runtime_cases() {
        for line in include_str!("../fixtures/identifier-paths.tsv").lines() {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let mut columns = line.split('\t');
            let backend = match columns.next().expect("backend") {
                "sqlite" => BackendKind::Sqlite,
                "postgres" => BackendKind::Postgres,
                "mssql" => BackendKind::Mssql,
                other => panic!("unknown backend {other}"),
            };
            let input = columns.next().expect("input");
            let expected = columns.next().expect("expected");
            assert_eq!(
                backend_render_identifier_path(backend, input),
                expected,
                "{line}"
            );
            if backend == BackendKind::Mssql {
                assert_eq!(
                    backend_quote_identifier_path(backend, input),
                    expected,
                    "{line}"
                );
            }
        }
    }
}
