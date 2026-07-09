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

pub(crate) fn backend_database_type_tokens(backend: BackendKind) -> proc_macro2::TokenStream {
    let marker = backend_marker_tokens(backend);
    quote! { <#marker as ::graphql_orm::SqlxBackend>::Database }
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
        if identifier.starts_with('[') && identifier.ends_with(']') {
            identifier.to_string()
        } else {
            format!("[{}]", identifier.replace(']', "]]"))
        }
    } else {
        identifier.to_string()
    }
}

pub(crate) fn backend_quote_identifier_path(backend: BackendKind, identifier: &str) -> String {
    if backend == BackendKind::Mssql {
        identifier
            .split('.')
            .filter(|part| !part.is_empty())
            .map(|part| backend_quote_identifier(backend, part))
            .collect::<Vec<_>>()
            .join(".")
    } else {
        identifier.to_string()
    }
}
