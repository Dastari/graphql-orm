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

pub(crate) fn backend_current_timestamp_expr(
    backend: BackendKind,
    native_date: bool,
) -> &'static str {
    if native_date {
        match backend {
            BackendKind::Postgres => return "NOW()",
            BackendKind::Mssql => return "SYSUTCDATETIME()",
            BackendKind::Sqlite => {}
        }
    }
    backend_current_text_timestamp_expr(backend)
}

pub(crate) fn backend_current_text_timestamp_expr(backend: BackendKind) -> &'static str {
    match backend {
        BackendKind::Postgres => {
            r#"to_char(NOW() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')"#
        }
        BackendKind::Mssql => "(CONVERT(varchar(27), SYSUTCDATETIME(), 126) + 'Z')",
        BackendKind::Sqlite => "strftime('%Y-%m-%dT%H:%M:%f000Z','now')",
    }
}

pub(crate) fn backend_current_offset_datetime_expr(backend: BackendKind) -> &'static str {
    match backend {
        BackendKind::Postgres => "NOW()",
        BackendKind::Mssql => "SYSDATETIMEOFFSET()",
        BackendKind::Sqlite => backend_current_text_timestamp_expr(backend),
    }
}

pub(crate) fn backend_quote_identifier(backend: BackendKind, identifier: &str) -> String {
    if backend != BackendKind::Mssql || backend_is_one_delimited_identifier(backend, identifier) {
        identifier.to_string()
    } else {
        format!("[{}]", identifier.replace(']', "]]"))
    }
}

/// Opening delimiter, closing delimiter, and whether a doubled closing
/// delimiter stands for one inside the name, matching the runtime
/// `DatabaseBackend` dialect.
fn backend_identifier_delimiters(backend: BackendKind) -> &'static [(char, char, bool)] {
    match backend {
        // SQLite reads `[...]` up to the first `]`, with no escape.
        BackendKind::Sqlite => &[('"', '"', true), ('`', '`', true), ('[', ']', false)],
        BackendKind::Postgres => &[('"', '"', true)],
        BackendKind::Mssql => &[('[', ']', true), ('"', '"', true)],
    }
}

/// Whether `identifier` is exactly one well-formed delimited identifier for
/// `backend`, the only input passed through unchanged (#782). It contains no
/// NUL, and its name may be empty only on SQLite. Matches the runtime
/// `DatabaseBackend` rule; the shared `identifier-paths.tsv` cases keep them
/// aligned.
pub(crate) fn backend_is_one_delimited_identifier(backend: BackendKind, identifier: &str) -> bool {
    if identifier.contains('\0') {
        return false;
    }
    let mut characters = identifier.chars();
    let Some(open) = characters.next() else {
        return false;
    };
    let Some(&(_, close, doubled)) = backend_identifier_delimiters(backend)
        .iter()
        .find(|(candidate, _, _)| *candidate == open)
    else {
        return false;
    };
    let mut empty = true;
    while let Some(character) = characters.next() {
        if character != close {
            empty = false;
            continue;
        }
        if doubled && characters.clone().next() == Some(close) {
            characters.next();
            empty = false;
            continue;
        }
        return (!empty || backend == BackendKind::Sqlite) && characters.next().is_none();
    }
    false
}

/// Split a SQL identifier path from the start of `input`. Each part must be
/// a bare token or one well-formed delimited identifier.
pub(crate) fn identifier_path_prefix(backend: BackendKind, input: &str) -> Option<(&str, &str)> {
    let mut rest = input;
    loop {
        let first = rest.chars().next()?;
        let length = if let Some(&(_, close, doubled)) = backend_identifier_delimiters(backend)
            .iter()
            .find(|(open, _, _)| *open == first)
        {
            let mut characters = rest.char_indices().skip(1).peekable();
            let end = loop {
                let (offset, character) = characters.next()?;
                if character == close {
                    if doubled && characters.peek().is_some_and(|&(_, next)| next == close) {
                        characters.next();
                    } else {
                        break offset + close.len_utf8();
                    }
                }
            };
            if !backend_is_one_delimited_identifier(backend, &rest[..end]) {
                return None;
            }
            end
        } else {
            let starts_identifier = |ch: char| {
                ch == '_'
                    || ch.is_ascii_alphabetic()
                    || (!ch.is_ascii() && !ch.is_whitespace())
                    || (backend == BackendKind::Mssql && matches!(ch, '#' | '@'))
            };
            if !starts_identifier(first) {
                return None;
            }
            rest.find(|ch: char| !starts_identifier(ch) && !ch.is_ascii_digit() && ch != '$')
                .unwrap_or(rest.len())
        };
        rest = &rest[length..];
        match rest.strip_prefix('.') {
            Some(next_part) => rest = next_part,
            None => return Some((&input[..input.len() - rest.len()], rest)),
        }
    }
}

/// Refuse a declared identifier path with a part that starts and ends with a
/// matching delimiter pair but is not exactly one well-formed, non-empty
/// delimited identifier for `backend`, such as `"a"b"`, `[a]]` or
/// `[a] , [b]` (#782). Earlier releases wrote such a part into SQL unchanged,
/// where it read as malformed SQL or as more than one token. Every other
/// declared name is accepted and rendered exactly as before.
pub(crate) fn validate_declared_identifier_path(
    backend: BackendKind,
    path: &str,
    span: proc_macro2::Span,
    attribute: &str,
) -> syn::Result<()> {
    if path.contains('\0') {
        return Err(syn::Error::new(
            span,
            format!(
                "{attribute} `{}` contains NUL, which no backend accepts in a name",
                path.escape_debug()
            ),
        ));
    }
    for part in identifier_path_parts(backend, path).filter(|part| !part.is_empty()) {
        let looks_delimited = backend_identifier_delimiters(backend)
            .iter()
            .any(|(open, close, _)| part.starts_with(*open) && part.ends_with(*close));
        if looks_delimited && !backend_is_one_delimited_identifier(backend, part) {
            return Err(syn::Error::new(
                span,
                format!(
                    "{attribute} part `{part}` starts and ends with a delimiter but is not one \
                     well-formed delimited identifier for the {} backend; double every closing \
                     delimiter inside it, or write the name without delimiters",
                    backend.name()
                ),
            ));
        }
    }
    Ok(())
}

/// Require one physical column. An explicit SQL delimiter distinguishes a
/// literal dot from a table qualification.
pub(crate) fn validate_declared_column(
    backend: BackendKind,
    column: &str,
    span: proc_macro2::Span,
) -> syn::Result<()> {
    let (open, delimiters) = if backend == BackendKind::Mssql {
        ('[', "square brackets")
    } else {
        ('"', "double quotes")
    };
    if column.contains('.')
        && !(column.starts_with(open) && backend_is_one_delimited_identifier(backend, column))
    {
        return Err(syn::Error::new(
            span,
            format!(
                "db_column must name one physical column; remove the table qualifier or enclose the entire literal dotted name in {delimiters}"
            ),
        ));
    }
    validate_declared_identifier_path(backend, column, span, "db_column")
}

/// The name one path part denotes: the inside of a well-formed delimited
/// identifier, or the part itself.
pub(crate) fn identifier_part_name(backend: BackendKind, part: &str) -> String {
    if !backend_is_one_delimited_identifier(backend, part) {
        return part.to_owned();
    }
    let mut characters = part.chars();
    let open = characters.next().unwrap_or_default();
    let (_, close, doubled) = backend_identifier_delimiters(backend)
        .iter()
        .copied()
        .find(|(candidate, _, _)| *candidate == open)
        .unwrap_or((open, open, false));
    let inner: String = characters.collect();
    let inner = inner.strip_suffix(close).unwrap_or(&inner);
    if doubled {
        inner.replace(&format!("{close}{close}"), &close.to_string())
    } else {
        inner.to_owned()
    }
}

pub(crate) fn same_identifier_part(backend: BackendKind, left: &str, right: &str) -> bool {
    match backend {
        BackendKind::Postgres => {
            // A bare part PostgreSQL folds; a delimited or quoted part keeps
            // its case, as the runtime's `postgres_resolved_identifier`.
            let resolved = |part: &str| {
                if backend_is_one_delimited_identifier(backend, part) {
                    identifier_part_name(backend, part)
                } else if part.starts_with('"') || backend_render_identifier(backend, part) != part
                {
                    part.to_owned()
                } else {
                    part.to_ascii_lowercase()
                }
            };
            resolved(left) == resolved(right)
        }
        BackendKind::Sqlite => identifier_part_name(backend, left)
            .eq_ignore_ascii_case(&identifier_part_name(backend, right)),
        BackendKind::Mssql => {
            identifier_part_name(backend, left) == identifier_part_name(backend, right)
        }
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
// Slice only at dots outside a delimited part, reading each part with the
// backend's own delimited forms, as the runtime `DatabaseBackend` splitter does.
// Byte offsets are safe because every delimiter is ASCII.
/// Split SQL identifier paths while preserving quoted dots and escapes.
pub(crate) fn identifier_path_parts(
    backend: BackendKind,
    identifier: &str,
) -> impl Iterator<Item = &str> {
    let delimiters = backend_identifier_delimiters(backend);
    let bytes = identifier.as_bytes();
    let mut start = 0;
    let mut offset = 0;
    std::iter::from_fn(move || {
        let mut closing: Option<(u8, bool)> = None;
        while offset < bytes.len() {
            let character = bytes[offset];
            offset += 1;
            match closing {
                Some((close, doubled)) if character == close => {
                    if doubled && bytes.get(offset) == Some(&close) {
                        offset += 1;
                    } else {
                        closing = None;
                    }
                }
                Some(_) => {}
                None => {
                    if offset - 1 == start
                        && let Some(&(_, close, doubled)) = delimiters
                            .iter()
                            .find(|(open, _, _)| u32::from(*open) == u32::from(character))
                    {
                        closing = Some((close as u8, doubled));
                    } else if character == b'.' {
                        let part = &identifier[start..offset - 1];
                        start = offset;
                        return Some(part);
                    }
                }
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
    if backend_is_one_delimited_identifier(backend, identifier) {
        return identifier.to_string();
    }
    let mut characters = identifier.chars();
    let plain_start = |character: char| {
        character.is_ascii_alphabetic() || character == '_' || !character.is_ascii()
    };
    // SQLite reads a byte-order mark that starts a token as whitespace.
    let plain = characters.next().is_some_and(|first| {
        plain_start(first) && !(backend == BackendKind::Sqlite && first == '\u{feff}')
    }) && characters
        .all(|character| plain_start(character) || character.is_ascii_digit() || character == '$');
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
    identifier_path_parts(backend, identifier)
        .filter(|part| !part.is_empty())
        .map(|part| backend_render_identifier(backend, part))
        .collect::<Vec<_>>()
        .join(".")
}

/// Whether a declared table path's own name, compared without ASCII case and
/// with or without delimiters, is one that read-policy SQL generates:
/// `__gom_self`, `__gom_row_<n>`, `__gom_visibility_<n>`, `__gom_closure_<n>`
/// or `__gom_closure_<n>_<suffix>`. A table of such a name could take a row
/// alias's place or be shadowed by a closure's expression (#796). Other
/// names starting with `__gom_` are allowed: aliases are allocated around
/// them.
pub(crate) fn backend_table_name_is_reserved(backend: BackendKind, table: &str) -> bool {
    let numbered = |rest: &str| !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit());
    identifier_path_parts(backend, table)
        .filter(|part| !part.is_empty())
        .last()
        .is_some_and(|part| {
            let name = part
                .trim_start_matches(['"', '[', '`'])
                .trim_end_matches(['"', ']', '`'])
                .to_ascii_lowercase();
            if name == "__gom_self" {
                return true;
            }
            if let Some(rest) = name
                .strip_prefix("__gom_row_")
                .or_else(|| name.strip_prefix("__gom_visibility_"))
            {
                return numbered(rest);
            }
            name.strip_prefix("__gom_closure_").is_some_and(|rest| {
                let (level, suffix) = rest.split_once('_').unwrap_or((rest, "x"));
                numbered(level) && !suffix.is_empty()
            })
        })
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
        identifier_path_parts(backend, identifier)
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
    fn malformed_delimited_declared_names_are_refused() {
        let span = proc_macro2::Span::call_site();
        for (backend, path) in [
            (BackendKind::Sqlite, "\"a\"b\""),
            (BackendKind::Sqlite, "\"\"\""),
            (BackendKind::Sqlite, "app.[a]b]"),
            (BackendKind::Postgres, "\""),
            (BackendKind::Postgres, "\"\""),
            (BackendKind::Postgres, "\"a\" , \"b\""),
            (BackendKind::Mssql, "[a]]"),
            (BackendKind::Mssql, "[a] , [b]"),
            (BackendKind::Mssql, "[app].[]"),
            (BackendKind::Sqlite, "a\0b"),
            (BackendKind::Postgres, "\"a\0b\""),
            (BackendKind::Mssql, "[a\0b]"),
            // SQLite closes `[...]` at the first `]`.
            (BackendKind::Sqlite, "[a]]b]"),
        ] {
            let error = validate_declared_identifier_path(backend, path, span, "table")
                .expect_err(path)
                .to_string();
            assert!(
                error.contains("well-formed delimited identifier") || error.contains("NUL"),
                "{error}"
            );
        }
        for (backend, path) in [
            (BackendKind::Sqlite, "users"),
            (BackendKind::Sqlite, "\"a\"\"b\""),
            (BackendKind::Sqlite, "[a b]"),
            (BackendKind::Sqlite, "\"a"),
            (BackendKind::Postgres, "app.\"Mixed Case\""),
            (BackendKind::Postgres, "[a]"),
            (BackendKind::Mssql, "[app].[a]].b]"),
            (BackendKind::Mssql, "\"a\"\".b\""),
            (BackendKind::Mssql, "a]"),
            (BackendKind::Sqlite, "\"\""),
            (BackendKind::Sqlite, "app.[]"),
            // Not delimited forms on these backends.
            (BackendKind::Postgres, "[s.t"),
            (BackendKind::Mssql, "`a.b`"),
        ] {
            validate_declared_identifier_path(backend, path, span, "table").expect(path);
        }
    }

    #[test]
    fn dotted_columns_need_a_single_canonical_delimiter() {
        let span = proc_macro2::Span::call_site();
        for backend in [
            BackendKind::Sqlite,
            BackendKind::Postgres,
            BackendKind::Mssql,
        ] {
            for column in ["a.b", r#""a"."b""#, "[a].[b]", "a.", ".b", r#""a.b"#] {
                assert!(
                    validate_declared_column(backend, column, span).is_err(),
                    "{backend:?}: {column}"
                );
            }
            let literal = if backend == BackendKind::Mssql {
                "[a.b]]c]"
            } else {
                r#""a.b""c""#
            };
            validate_declared_column(backend, literal, span).unwrap();
        }
    }

    #[test]
    fn only_one_well_formed_delimited_identifier_passes_through() {
        assert_eq!(
            backend_render_identifier(BackendKind::Sqlite, "\"join\""),
            "\"join\""
        );
        assert_eq!(
            backend_render_identifier(BackendKind::Sqlite, "\"a\"b\""),
            "\"\"\"a\"\"b\"\"\""
        );
        assert_eq!(
            backend_render_identifier(BackendKind::Postgres, "\""),
            "\"\"\"\""
        );
        assert_eq!(
            backend_quote_identifier(BackendKind::Mssql, "[key]"),
            "[key]"
        );
        assert_eq!(
            backend_quote_identifier(BackendKind::Mssql, "[a]]"),
            "[[a]]]]]"
        );
        assert_eq!(
            backend_render_identifier(BackendKind::Mssql, "[a] , [b]"),
            "[[a]] , [b]]]"
        );
    }

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
