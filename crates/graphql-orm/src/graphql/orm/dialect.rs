use super::mssql_identifier_tables::{MSSQL_IDENTIFIER_PART, MSSQL_IDENTIFIER_START};

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum DatabaseBackend {
    Sqlite,
    Postgres,
    Mysql,
    Mssql,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SpatialPredicate {
    Equals,
    Disjoint,
    Intersects,
    Touches,
    Crosses,
    Within,
    Contains,
    Overlaps,
}

impl DatabaseBackend {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::Postgres => "postgres",
            Self::Mysql => "mysql",
            Self::Mssql => "mssql",
        }
    }

    pub const fn supports_native_spatial_predicates(self) -> bool {
        matches!(self, Self::Postgres)
    }
}

// Keep these canonical expressions aligned with the derive backend helpers.
pub(crate) fn implicit_timestamp_defaults(
    backend: DatabaseBackend,
) -> Option<(&'static str, &'static str)> {
    match backend {
        DatabaseBackend::Sqlite => Some(("unixepoch()", "strftime('%Y-%m-%dT%H:%M:%f000Z','now')")),
        DatabaseBackend::Postgres => Some((
            "EXTRACT(EPOCH FROM NOW())::bigint",
            r#"to_char(NOW() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')"#,
        )),
        DatabaseBackend::Mssql => Some((
            "DATEDIFF_BIG(second, '1970-01-01', SYSUTCDATETIME())",
            "CONVERT(varchar(27), SYSUTCDATETIME(), 126) + 'Z'",
        )),
        DatabaseBackend::Mysql => None,
    }
}

/// Canonicalize a column default expression for schema comparison and hashing.
///
/// SQLite stores `DEFAULT (unixepoch())` as `dflt_value = unixepoch()`, while
/// generated entity metadata may historically use either `unixepoch()` or
/// `(unixepoch())`. Semantically equivalent defaults must not produce false
/// `AlterColumn` steps when a managed schema is replaned after reopening a
/// file-backed database.
///
/// Rules (intentionally conservative):
/// - trim surrounding whitespace
/// - strip balanced outer parentheses that wrap the entire expression
/// - normalize SQL keyword defaults (`CURRENT_TIMESTAMP`, `NULL`, booleans)
/// - recognize PostgreSQL's deparsed form of the ORM's known epoch-second default
/// - leave string/blob literals and other non-keyword identifiers untouched
///
/// Apart from that exact known default, does **not** rewrite operators, function names, or argument order, so
/// genuinely different expressions remain distinct.
pub fn canonicalize_column_default_expression(default: &str) -> String {
    let mut value = default.trim().to_string();
    while is_fully_parenthesized(&value) {
        value = value[1..value.len() - 1].trim().to_string();
    }

    // PostgreSQL deparses the ORM's default with lowercase function/epoch names
    // and parentheses around EXTRACT before its bigint cast. Whitelist only these
    // exact equivalent forms; do not interpret arbitrary SQL or change units.
    if matches!(
        value.to_ascii_lowercase().as_str(),
        "extract(epoch from now())::bigint" | "(extract(epoch from now()))::bigint"
    ) {
        return "EXTRACT(EPOCH FROM NOW())::bigint".to_string();
    }

    let uppercase = value.to_ascii_uppercase();
    match uppercase.as_str() {
        "CURRENT_TIMESTAMP" | "CURRENT_DATE" | "CURRENT_TIME" | "NULL" => uppercase,
        "TRUE" | "FALSE" => value.to_ascii_lowercase(),
        _ => value,
    }
}

/// Return true when `value` is wrapped by a single pair of parentheses that
/// enclose the whole expression (depth never returns to zero before the end).
fn is_fully_parenthesized(value: &str) -> bool {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() < 2 || chars[0] != '(' || chars[chars.len() - 1] != ')' {
        return false;
    }

    let mut depth = 0i32;
    let mut in_single = false;
    let mut in_double = false;
    let mut i = 0usize;
    while i < chars.len() {
        let ch = chars[i];
        if in_single {
            if ch == '\'' {
                if i + 1 < chars.len() && chars[i + 1] == '\'' {
                    i += 2;
                    continue;
                }
                in_single = false;
            }
            i += 1;
            continue;
        }
        if in_double {
            if ch == '"' {
                if i + 1 < chars.len() && chars[i + 1] == '"' {
                    i += 2;
                    continue;
                }
                in_double = false;
            }
            i += 1;
            continue;
        }
        match ch {
            '\'' => in_single = true,
            '"' => in_double = true,
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 && i != chars.len() - 1 {
                    return false;
                }
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
        i += 1;
    }
    depth == 0
}

#[cfg(test)]
mod default_canonicalization_tests {
    use super::canonicalize_column_default_expression;

    #[test]
    fn strips_redundant_outer_parens_for_epoch_defaults() {
        assert_eq!(
            canonicalize_column_default_expression("(unixepoch())"),
            "unixepoch()"
        );
        assert_eq!(
            canonicalize_column_default_expression("unixepoch()"),
            "unixepoch()"
        );
        assert_eq!(
            canonicalize_column_default_expression("((unixepoch()))"),
            "unixepoch()"
        );
        assert_eq!(
            canonicalize_column_default_expression("  ( date('now') )  "),
            "date('now')"
        );
    }

    #[test]
    fn preserves_meaningful_structure_and_literals() {
        assert_eq!(canonicalize_column_default_expression("1+2"), "1+2");
        assert_eq!(canonicalize_column_default_expression("(1+2)"), "1+2");
        assert_eq!(
            canonicalize_column_default_expression("date('now', '-1 days')"),
            "date('now', '-1 days')"
        );
        assert_eq!(
            canonicalize_column_default_expression("'hello (world)'"),
            "'hello (world)'"
        );
        // Parentheses that do not wrap the entire expression stay put.
        assert_eq!(canonicalize_column_default_expression("(1+2)*3"), "(1+2)*3");
    }

    #[test]
    fn normalizes_keywords_and_booleans() {
        assert_eq!(
            canonicalize_column_default_expression("current_timestamp"),
            "CURRENT_TIMESTAMP"
        );
        assert_eq!(canonicalize_column_default_expression("TRUE"), "true");
        assert_eq!(canonicalize_column_default_expression("null"), "NULL");
    }

    #[test]
    fn different_functions_remain_distinct() {
        assert_ne!(
            canonicalize_column_default_expression("unixepoch()"),
            canonicalize_column_default_expression("date('now')")
        );
        assert_ne!(
            canonicalize_column_default_expression("(unixepoch())"),
            canonicalize_column_default_expression("(date('now'))")
        );
    }
}

/// One lexical token of a SQL fragment: see [`scan_sql`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SqlTokenKind {
    /// A bare word: an identifier, keyword, number, or (on SQL Server and
    /// SQLite) a `@name`, `#name`, `:name` variable or parameter.
    Word,
    /// A delimited identifier: `"..."`, and per dialect `` `...` `` or
    /// `[...]`.
    QuotedIdentifier,
    /// A string constant: `'...'`, PostgreSQL `E'...'` or `$tag$...$tag$`.
    StringLiteral,
    /// A line or block comment.
    Comment,
    /// A bind placeholder: `?`, `?<digits>`, `$<digits>` or `@p<digits>`.
    Placeholder,
    /// Any other single byte outside whitespace.
    Symbol,
}

/// A token's kind and byte range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SqlToken {
    pub(crate) kind: SqlTokenKind,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

/// A construct [`scan_sql`] could not read with certainty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SqlLexError {
    /// A string constant with no closing quote.
    UnterminatedString,
    /// A delimited identifier with no closing delimiter.
    UnterminatedIdentifier,
    /// A block comment with no closing `*/`.
    UnterminatedComment,
    /// A line comment that runs to the end of the fragment, so it would
    /// comment out whatever SQL follows the fragment.
    OpenLineComment,
    /// A PostgreSQL dollar-quoted string with no closing delimiter.
    UnterminatedDollarQuote,
}

/// The one dialect-aware lexical pass over a SQL fragment. Placeholder
/// counting and rebasing, and the read-predicate row binder, qualifier
/// check, alias allocator and nesting measure, all read SQL through it, so
/// they agree on what is code and what is quoted text.
///
/// `visit` is called with each token in order; whitespace is skipped. Per
/// dialect it reads:
///
/// - line comments (`-- ...`) and block comments, nested on PostgreSQL and
///   SQL Server and ending at the first `*/` on SQLite;
/// - `'...'` strings with doubled quotes, PostgreSQL `E'...'` escape strings
///   (backslash escapes) and `$tag$...$tag$` dollar-quoted strings. An
///   ordinary PostgreSQL string is read with `standard_conforming_strings`
///   on, the server default, which generated SQL such as `ESCAPE '\'`
///   already requires: a backslash in it is an ordinary character;
/// - `"..."` identifiers with doubled quotes, SQL Server `[...]` identifiers
///   (`]]` escapes), SQLite `[...]` identifiers (closed by the first `]`) and
///   SQLite backticks. On PostgreSQL `[` is an array subscript, a symbol;
/// - placeholders: `?` (with optional trailing digits), `$<digits>` or
///   `@p<digits>` (either case). `$`/`@p` followed by no digits, or attached
///   to an identifier (`col$1`, `@page`, `@@p1`), is ordinary text.
///
/// A construct that cannot be read with certainty is still consumed, to the
/// end of the fragment when it is unterminated, and the first such construct
/// is returned as the error. Placeholder consumers ignore it; read
/// predicates refuse the fragment.
pub(crate) fn scan_sql(
    backend: DatabaseBackend,
    sql: &str,
    mut visit: impl FnMut(SqlToken),
) -> Result<(), SqlLexError> {
    let bytes = sql.as_bytes();
    let postgres = backend == DatabaseBackend::Postgres;
    let mssql = backend == DatabaseBackend::Mssql;
    let sqlite = backend == DatabaseBackend::Sqlite;
    let mut uncertain: Option<SqlLexError> = None;
    let mut flag = |error: SqlLexError| {
        uncertain.get_or_insert(error);
    };
    // Skip to just after the closing delimiter; a doubled one is an escape
    // when `doubled`. The second value is false when no delimiter closed it.
    let skip_quoted = |mut i: usize, close: u8, doubled: bool, backslash_escapes: bool| {
        while i < bytes.len() {
            if backslash_escapes && bytes[i] == b'\\' {
                i = (i + 2).min(bytes.len());
            } else if bytes[i] == close {
                i += 1;
                if doubled && bytes.get(i) == Some(&close) {
                    i += 1;
                } else {
                    return (i, true);
                }
            } else {
                i += 1;
            }
        }
        (i, false)
    };
    let digits_end = |mut i: usize| {
        while bytes.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        i
    };
    // SQL Server reads `@` and `#` inside identifiers and variables.
    let word_byte =
        |byte: u8| postgres_identifier_byte(byte) || (mssql && matches!(byte, b'@' | b'#'));
    let word_end = |mut i: usize| {
        while bytes.get(i).is_some_and(|byte| word_byte(*byte)) {
            i += 1;
        }
        i
    };
    let mut i = 0usize;
    while i < bytes.len() {
        let start = i;
        let attached = i > 0 && postgres_identifier_byte(bytes[i - 1]);
        let kind = match bytes[i] {
            byte if byte.is_ascii_whitespace() => {
                i += 1;
                continue;
            }
            b'\'' => {
                let escaped = postgres
                    && i > 0
                    && matches!(bytes[i - 1], b'e' | b'E')
                    && (i == 1 || !postgres_identifier_byte(bytes[i - 2]));
                let (end, closed) = skip_quoted(i + 1, b'\'', true, escaped);
                if !closed {
                    flag(SqlLexError::UnterminatedString);
                }
                i = end;
                SqlTokenKind::StringLiteral
            }
            b'"' => {
                let (end, closed) = skip_quoted(i + 1, b'"', true, false);
                if !closed {
                    flag(SqlLexError::UnterminatedIdentifier);
                }
                i = end;
                SqlTokenKind::QuotedIdentifier
            }
            b'`' if matches!(backend, DatabaseBackend::Sqlite | DatabaseBackend::Mysql) => {
                let (end, closed) = skip_quoted(i + 1, b'`', true, false);
                if !closed {
                    flag(SqlLexError::UnterminatedIdentifier);
                }
                i = end;
                SqlTokenKind::QuotedIdentifier
            }
            b'[' if mssql || sqlite => {
                // SQL Server escapes `]` as `]]`; SQLite closes at the first.
                let (end, closed) = skip_quoted(i + 1, b']', mssql, false);
                if !closed {
                    flag(SqlLexError::UnterminatedIdentifier);
                }
                i = end;
                SqlTokenKind::QuotedIdentifier
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                i += 2;
                while i < bytes.len() && !matches!(bytes[i], b'\n' | b'\r') {
                    i += 1;
                }
                if i == bytes.len() {
                    flag(SqlLexError::OpenLineComment);
                }
                SqlTokenKind::Comment
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                // PostgreSQL and SQL Server nest block comments; SQLite ends
                // one at the first `*/`.
                let nests = !sqlite;
                i += 2;
                let mut depth = 1usize;
                while i < bytes.len() && depth > 0 {
                    if nests && bytes.get(i..i + 2) == Some(b"/*") {
                        depth += 1;
                        i += 2;
                    } else if bytes.get(i..i + 2) == Some(b"*/") {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                if depth > 0 {
                    flag(SqlLexError::UnterminatedComment);
                }
                SqlTokenKind::Comment
            }
            b'?' => {
                i = digits_end(i + 1);
                SqlTokenKind::Placeholder
            }
            b'$' if !attached => {
                let end = digits_end(i + 1);
                if end > i + 1 {
                    i = end;
                    SqlTokenKind::Placeholder
                } else {
                    // A dollar-quoted string: `$$ ... $$` or `$tag$ ... $tag$`.
                    let mut tag_end = i + 1;
                    if bytes
                        .get(tag_end)
                        .is_some_and(|ch| ch.is_ascii_alphabetic() || *ch == b'_' || *ch >= 0x80)
                    {
                        tag_end += 1;
                        while bytes.get(tag_end).is_some_and(|ch| {
                            ch.is_ascii_alphanumeric() || *ch == b'_' || *ch >= 0x80
                        }) {
                            tag_end += 1;
                        }
                    }
                    if postgres && bytes.get(tag_end) == Some(&b'$') {
                        let delimiter = &sql[i..=tag_end];
                        let body = tag_end + 1;
                        i = match sql[body..].find(delimiter) {
                            Some(offset) => body + offset + delimiter.len(),
                            None => {
                                flag(SqlLexError::UnterminatedDollarQuote);
                                bytes.len()
                            }
                        };
                        SqlTokenKind::StringLiteral
                    } else {
                        // `$name`, a SQLite parameter or SQL Server
                        // pseudo-column: one word, never a bare identifier.
                        i = word_end(i + 1);
                        if i == start + 1 {
                            SqlTokenKind::Symbol
                        } else {
                            SqlTokenKind::Word
                        }
                    }
                }
            }
            b'@' if !attached
                && (i == 0 || bytes[i - 1] != b'@')
                && bytes
                    .get(i + 1)
                    .is_some_and(|ch| ch.eq_ignore_ascii_case(&b'p'))
                && digits_end(i + 2) > i + 2
                && !bytes
                    .get(digits_end(i + 2))
                    .is_some_and(|ch| postgres_identifier_byte(*ch)) =>
            {
                i = digits_end(i + 2);
                SqlTokenKind::Placeholder
            }
            // A SQLite `@name` or `:name` parameter is one word, so the
            // name is never read as a bare identifier.
            b'@' | b':'
                if sqlite
                    && (i == 0 || bytes[i - 1] != bytes[i])
                    && bytes
                        .get(i + 1)
                        .is_some_and(|byte| postgres_identifier_byte(*byte)) =>
            {
                i = word_end(i + 1);
                SqlTokenKind::Word
            }
            byte if word_byte(byte) => {
                i = word_end(i);
                SqlTokenKind::Word
            }
            _ => {
                // One whole character, so a token never splits UTF-8.
                i += 1;
                while i < bytes.len() && (bytes[i] & 0xC0) == 0x80 {
                    i += 1;
                }
                SqlTokenKind::Symbol
            }
        };
        visit(SqlToken {
            kind,
            start,
            end: i,
        });
    }
    match uncertain {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// Byte ranges of the bind placeholders in a SQL fragment, in order.
///
/// This is the one lexical pass every placeholder consumer shares: counting a
/// fragment's binds and rebasing them must agree, or values are attached to
/// the wrong clause. See [`scan_sql`] for what is read as quoted text.
pub(crate) fn placeholder_spans(backend: DatabaseBackend, sql: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    // An unterminated construct runs to the end of the fragment; the
    // placeholders before it are still the fragment's binds.
    let _ = scan_sql(backend, sql, |token| {
        if token.kind == SqlTokenKind::Placeholder {
            spans.push((token.start, token.end));
        }
    });
    spans
}

fn normalize_sql_placeholders(backend: DatabaseBackend, sql: &str, start_index: usize) -> String {
    if !matches!(backend, DatabaseBackend::Postgres | DatabaseBackend::Mssql) {
        return sql.to_string();
    }

    let mut out = String::with_capacity(sql.len() + 16);
    let mut previous = 0usize;
    for (offset, (start, end)) in placeholder_spans(backend, sql).into_iter().enumerate() {
        out.push_str(&sql[previous..start]);
        out.push_str(&backend.placeholder(start_index + offset));
        previous = end;
    }
    out.push_str(&sql[previous..]);
    out
}

// Complete PostgreSQL statements already carry binding-slot identity. Static
// fragments do not: generated field fragments can start at arbitrary indices,
// and nested filters can restart numbering. Keep their existing occurrence
// rebasing separate from this final driver-boundary normalization.
pub(crate) fn normalize_postgres_statement(sql: &str, start_index: usize) -> String {
    let bytes = sql.as_bytes();
    let mut parameters = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' | b'"' => {
                let quote = bytes[i];
                let escaped = quote == b'\''
                    && i > 0
                    && matches!(bytes[i - 1], b'e' | b'E')
                    && (i == 1 || !postgres_identifier_byte(bytes[i - 2]));
                i += 1;
                while i < bytes.len() {
                    if escaped && bytes[i] == b'\\' {
                        i = (i + 2).min(bytes.len());
                    } else if bytes[i] == quote {
                        i += 1;
                        if bytes.get(i) == Some(&quote) {
                            i += 1;
                        } else {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                i += 2;
                while i < bytes.len() && !matches!(bytes[i], b'\n' | b'\r') {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                let mut depth = 1usize;
                while i < bytes.len() && depth > 0 {
                    if bytes.get(i..i + 2) == Some(b"/*") {
                        depth += 1;
                        i += 2;
                    } else if bytes.get(i..i + 2) == Some(b"*/") {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            b'$' if i == 0 || !postgres_identifier_byte(bytes[i - 1]) => {
                let start = i;
                i += 1;
                if bytes.get(i).is_some_and(u8::is_ascii_digit) {
                    while bytes.get(i).is_some_and(u8::is_ascii_digit) {
                        i += 1;
                    }
                    parameters.push((start, i));
                } else {
                    // PostgreSQL dollar-quoted strings ($$ or $tag$).
                    let tag_start = i;
                    if bytes
                        .get(i)
                        .is_some_and(|ch| ch.is_ascii_alphabetic() || *ch == b'_' || *ch >= 0x80)
                    {
                        i += 1;
                        while bytes.get(i).is_some_and(|ch| {
                            ch.is_ascii_alphanumeric() || *ch == b'_' || *ch >= 0x80
                        }) {
                            i += 1;
                        }
                    }
                    if bytes.get(i) == Some(&b'$') {
                        i += 1;
                        let delimiter = &sql[start..i];
                        i = sql[i..]
                            .find(delimiter)
                            .map_or(bytes.len(), |offset| i + offset + delimiter.len());
                    } else {
                        // Not a parameter or quote delimiter; keep the original token.
                        i = tag_start;
                    }
                }
            }
            b'?' => {
                return normalize_sql_placeholders(DatabaseBackend::Postgres, sql, start_index);
            }
            b'@' if bytes
                .get(i + 1)
                .is_some_and(|ch| ch.eq_ignore_ascii_case(&b'p')) =>
            {
                return normalize_sql_placeholders(DatabaseBackend::Postgres, sql, start_index);
            }
            _ => i += 1,
        }
    }
    let offset = start_index.saturating_sub(1);
    if offset == 0 {
        return sql.to_owned();
    }
    let mut out = String::with_capacity(sql.len());
    let mut previous = 0;
    for (start, end) in parameters {
        out.push_str(&sql[previous..start]);
        // Leave malformed/overflowing indices for the driver's normal safe error
        // path rather than panicking or guessing a different binding identity.
        if let Some(index) = sql[start + 1..end]
            .parse::<usize>()
            .ok()
            .filter(|n| *n > 0)
            .and_then(|n| n.checked_add(offset))
        {
            out.push_str(&DatabaseBackend::Postgres.placeholder(index));
        } else {
            out.push_str(&sql[start..end]);
        }
        previous = end;
    }
    out.push_str(&sql[previous..]);
    out
}

fn postgres_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$') || byte >= 0x80
}

#[cfg(test)]
mod sql_lexer_tests {
    use super::DatabaseBackend::{self, Mssql, Postgres, Sqlite};
    use super::{SqlLexError, SqlTokenKind, scan_sql};

    /// The bare words of `sql`, and whether it was read with certainty.
    fn words(backend: DatabaseBackend, sql: &str) -> (Vec<&str>, Result<(), SqlLexError>) {
        let mut words = Vec::new();
        let result = scan_sql(backend, sql, |token| {
            if token.kind == SqlTokenKind::Word {
                words.push(&sql[token.start..token.end]);
            }
        });
        (words, result)
    }

    fn kinds(backend: DatabaseBackend, sql: &str) -> Vec<(SqlTokenKind, &str)> {
        let mut tokens = Vec::new();
        scan_sql(backend, sql, |token| {
            tokens.push((token.kind, &sql[token.start..token.end]));
        })
        .unwrap();
        tokens
    }

    const ALL: [DatabaseBackend; 3] = [Sqlite, Postgres, Mssql];

    #[test]
    fn comments_hide_their_text_and_nothing_after_them() {
        for backend in ALL {
            // An apostrophe in a comment opens no string.
            assert_eq!(
                words(backend, "a -- it's\n AND b"),
                (vec!["a", "AND", "b"], Ok(())),
                "{backend:?}"
            );
            assert_eq!(
                words(backend, "a /* it's */ AND b"),
                (vec!["a", "AND", "b"], Ok(())),
                "{backend:?}"
            );
            assert_eq!(
                words(backend, "a -- runs to the end"),
                (vec!["a"], Err(SqlLexError::OpenLineComment)),
                "{backend:?}"
            );
            assert_eq!(
                words(backend, "a /* never closed"),
                (vec!["a"], Err(SqlLexError::UnterminatedComment)),
                "{backend:?}"
            );
        }
        // PostgreSQL and SQL Server nest block comments.
        for backend in [Postgres, Mssql] {
            assert_eq!(
                words(backend, "a /* x /* it's */ y */ b"),
                (vec!["a", "b"], Ok(())),
                "{backend:?}"
            );
            assert_eq!(
                words(backend, "a /* x /* y */ b"),
                (vec!["a"], Err(SqlLexError::UnterminatedComment)),
                "{backend:?}"
            );
        }
        // SQLite ends one at the first `*/`.
        assert_eq!(words(Sqlite, "a /* x /* y */ b"), (vec!["a", "b"], Ok(())));
    }

    #[test]
    fn strings_end_at_their_own_quote() {
        for backend in ALL {
            assert_eq!(
                words(backend, "a = 'it''s -- b /* c' AND d"),
                (vec!["a", "AND", "d"], Ok(())),
                "{backend:?}"
            );
            assert_eq!(
                words(backend, "a = 'open AND d"),
                (vec!["a"], Err(SqlLexError::UnterminatedString)),
                "{backend:?}"
            );
        }
        // A PostgreSQL escape string reads `\'` as a quote character.
        assert_eq!(
            words(Postgres, r"a = E'x\' b' AND d"),
            (vec!["a", "E", "AND", "d"], Ok(()))
        );
        assert_eq!(
            words(Postgres, r"a = e'x\\' AND d"),
            (vec!["a", "e", "AND", "d"], Ok(()))
        );
        // Elsewhere a backslash is an ordinary character.
        for backend in [Sqlite, Mssql] {
            assert_eq!(
                words(backend, r"a = E'x\' AND d"),
                (vec!["a", "E", "AND", "d"], Ok(())),
                "{backend:?}"
            );
        }
        // An ordinary PostgreSQL string is read as the server reads it by
        // default, and as generated `ESCAPE '\'` needs: no backslash escape.
        assert_eq!(
            words(Postgres, r"a LIKE b ESCAPE '\' AND d"),
            (vec!["a", "LIKE", "b", "ESCAPE", "AND", "d"], Ok(()))
        );
    }

    #[test]
    fn postgres_dollar_quotes_are_strings() {
        assert_eq!(
            words(Postgres, "a = $$ it's b $$ AND c = $q$ d $$ e $q$ AND f"),
            (vec!["a", "AND", "c", "AND", "f"], Ok(()))
        );
        assert_eq!(
            words(Postgres, "a = $q$ never closed $$"),
            (vec!["a"], Err(SqlLexError::UnterminatedDollarQuote))
        );
        assert_eq!(
            kinds(Postgres, "$1 $q$x$q$"),
            vec![
                (SqlTokenKind::Placeholder, "$1"),
                (SqlTokenKind::StringLiteral, "$q$x$q$"),
            ]
        );
        // Not a quote on the other backends: `$q$` is one word.
        for backend in [Sqlite, Mssql] {
            assert_eq!(
                words(backend, "a = $q$ b $q$"),
                (vec!["a", "$q$", "b", "$q$"], Ok(())),
                "{backend:?}"
            );
        }
    }

    #[test]
    fn delimited_identifiers_follow_each_dialect() {
        for backend in ALL {
            assert_eq!(
                words(backend, "\"a\"\"b -- c\" = d"),
                (vec!["d"], Ok(())),
                "{backend:?}"
            );
            assert_eq!(
                words(backend, "\"open = d"),
                (vec![], Err(SqlLexError::UnterminatedIdentifier)),
                "{backend:?}"
            );
        }
        // SQL Server brackets escape `]` as `]]`.
        assert_eq!(words(Mssql, "[a]]b 'c] = d"), (vec!["d"], Ok(())));
        assert_eq!(
            words(Mssql, "[open = d"),
            (vec![], Err(SqlLexError::UnterminatedIdentifier))
        );
        // SQLite brackets close at the first `]`.
        assert_eq!(words(Sqlite, "[a 'b] = d"), (vec!["d"], Ok(())));
        assert_eq!(words(Sqlite, "[a]]b = d"), (vec!["b", "d"], Ok(())));
        // On PostgreSQL `[` subscripts an array.
        assert_eq!(
            words(Postgres, "tags[b] = d"),
            (vec!["tags", "b", "d"], Ok(()))
        );
        assert_eq!(
            kinds(Postgres, "t[1]"),
            vec![
                (SqlTokenKind::Word, "t"),
                (SqlTokenKind::Symbol, "["),
                (SqlTokenKind::Word, "1"),
                (SqlTokenKind::Symbol, "]"),
            ]
        );
        // SQLite backticks quote a name; elsewhere a backtick is a symbol.
        assert_eq!(words(Sqlite, "`a``b 'c` = d"), (vec!["d"], Ok(())));
        assert_eq!(
            words(Sqlite, "`open = d"),
            (vec![], Err(SqlLexError::UnterminatedIdentifier))
        );
        for backend in [Postgres, Mssql] {
            assert_eq!(
                words(backend, "`a` = d"),
                (vec!["a", "d"], Ok(())),
                "{backend:?}"
            );
        }
    }

    #[test]
    fn words_keep_their_prefixes_and_placeholders_stay_tokens() {
        for backend in ALL {
            assert_eq!(
                kinds(backend, "a.b = ? AND c$1 = ?2"),
                vec![
                    (SqlTokenKind::Word, "a"),
                    (SqlTokenKind::Symbol, "."),
                    (SqlTokenKind::Word, "b"),
                    (SqlTokenKind::Symbol, "="),
                    (SqlTokenKind::Placeholder, "?"),
                    (SqlTokenKind::Word, "AND"),
                    (SqlTokenKind::Word, "c$1"),
                    (SqlTokenKind::Symbol, "="),
                    (SqlTokenKind::Placeholder, "?2"),
                ],
                "{backend:?}"
            );
            assert_eq!(
                kinds(backend, "@p1 $2"),
                vec![
                    (SqlTokenKind::Placeholder, "@p1"),
                    (SqlTokenKind::Placeholder, "$2"),
                ],
                "{backend:?}"
            );
        }
        // A SQL Server variable or temporary table, and a SQLite named
        // parameter, is one word with its prefix.
        assert_eq!(
            words(Mssql, "@name = #temp.x AND @@rowcount > 0").0,
            vec!["@name", "#temp", "x", "AND", "@@rowcount", "0"]
        );
        assert_eq!(
            words(Sqlite, "@name = :other AND $third").0,
            vec!["@name", ":other", "AND", "$third"]
        );
        // On PostgreSQL `@` is an operator and `::` a cast.
        assert_eq!(
            words(Postgres, "@ name = x::text").0,
            vec!["name", "x", "text"]
        );
        assert_eq!(
            words(Postgres, "ünïcode = 'ü' AND b").0,
            vec!["ünïcode", "AND", "b"]
        );
    }
}

#[cfg(test)]
mod postgres_statement_tests {
    use super::{DatabaseBackend, normalize_postgres_statement, normalize_sql_placeholders};

    #[test]
    fn native_identity_and_rebasing_are_idempotent_at_the_binding_boundary() {
        for (sql, start, expected) in [
            ("SELECT $1, $1", 1, "SELECT $1, $1"),
            ("SELECT $2, $1, $2", 3, "SELECT $4, $3, $4"),
            ("SELECT $01, $1", 3, "SELECT $3, $3"),
            ("SELECT $10, $10, $1", 1, "SELECT $10, $10, $1"),
            ("SELECT ?, ?", 3, "SELECT $3, $4"),
        ] {
            let normalized = normalize_postgres_statement(sql, start);
            assert_eq!(normalized, expected);
            assert_eq!(normalize_postgres_statement(&normalized, 1), expected);
        }
    }

    #[test]
    fn anonymous_mixed_and_static_fragment_rebasing_remain_compatible() {
        for sql in ["SELECT ?, ?", "SELECT $8, ?, @P2, $8", "SELECT ?1, @p9, $4"] {
            assert_eq!(
                normalize_postgres_statement(sql, 4),
                normalize_sql_placeholders(DatabaseBackend::Postgres, sql, 4)
            );
        }
        assert_eq!(
            normalize_sql_placeholders(DatabaseBackend::Postgres, "value = $9", 2),
            "value = $2"
        );
    }

    /// Counting a fragment's binds and rebasing them read the same tokens
    /// (#458): each fragment here has exactly the stated number of
    /// placeholders for both, on every backend that rewrites them.
    #[test]
    fn counting_and_rebasing_agree_on_every_fragment() {
        use super::placeholder_spans;
        use DatabaseBackend::{Mssql, Postgres, Sqlite};
        let all = [Sqlite, Postgres, Mssql];
        let cases: &[(&str, &[DatabaseBackend], usize)] = &[
            ("a = ? AND b = ?", &all, 2),
            (
                "a = $1 AND b = $2 AND c = @P3 AND d = @p4 AND e = ?5",
                &all,
                5,
            ),
            ("a = '?' AND b = 'it''s $1 @p2' AND c = ?", &all, 1),
            ("\"odd?\"\"$1\" = ?", &all, 1),
            ("a = ? -- trailing ? $1 @p1\n AND b = ?", &all, 2),
            ("a = ? /* ? $1 @p1 */ AND b = ?", &all, 2),
            // A bare `$` or `@p`, a variable and a system variable are text.
            (
                "a = $ AND b = @p AND c = @page AND d = @@p1 AND e = @p1x",
                &all,
                0,
            ),
            ("col$1 = ? AND col@p1 = ?", &all, 2),
            // Brackets quote a name here ...
            ("[what?] = ? AND [a]]$1] = ?", &[Mssql], 2),
            // ... and here, where the first `]` closes the name.
            ("[what?] = ? AND [a$1] = ?", &[Sqlite], 2),
            ("[what?] = ? AND [a]]$1] = ?", &[Sqlite], 3),
            // ... and subscript an array here.
            ("tags[?] = ? AND tags[$1] = $2", &[Postgres], 4),
            (
                "a = $$ ? $1 $$ AND b = $q$ ? $q$ AND c = E'\\'?' AND d = ?",
                &[Postgres],
                1,
            ),
            (
                "a = ? /* outer /* inner ? */ still ? */ AND b = ?",
                &[Postgres, Mssql],
                2,
            ),
            // A dollar sign before a non-tag is not a quote on other backends.
            ("a = $x$ AND b = ?", &[Sqlite, Mssql], 1),
            ("ünïcode = '?' AND b = ?", &all, 1),
        ];
        for (sql, backends, expected) in cases {
            for backend in *backends {
                let spans = placeholder_spans(*backend, sql);
                assert_eq!(spans.len(), *expected, "{backend:?}: {sql}");
                assert_eq!(
                    crate::graphql::orm::count_placeholders_for(*backend, sql),
                    *expected,
                    "{backend:?}: {sql}"
                );
                if *backend == Sqlite {
                    assert_eq!(normalize_sql_placeholders(*backend, sql, 7), *sql);
                    continue;
                }
                // Rebasing rewrites exactly the counted tokens, in order, and
                // its output counts the same.
                let rebased = normalize_sql_placeholders(*backend, sql, 7);
                let rewritten = placeholder_spans(*backend, &rebased);
                assert_eq!(rewritten.len(), *expected, "{backend:?}: {rebased}");
                for (offset, (start, end)) in rewritten.into_iter().enumerate() {
                    assert_eq!(
                        &rebased[start..end],
                        super::SqlDialect::placeholder(backend, 7 + offset),
                        "{backend:?}: {rebased}"
                    );
                }
            }
        }
    }

    #[test]
    fn native_scanner_skips_quotes_identifiers_comments_and_dollar_strings() {
        let sql = "SELECT $2, '$1?', 'it''s $9?', \"$1?\", \"a\"\"$2\", $$ $9 ? $$, $tag$ $4 ? $tag$, $π$ $8 ? $π$, E'\\\'$8?', e'\\\\$9', col$1, π$3, $2 -- $9 ?\n/* $7 /* $6 ? */ */";
        let expected = sql
            .replacen("SELECT $2", "SELECT $4", 1)
            .replace(", $2 --", ", $4 --");
        assert_eq!(normalize_postgres_statement(sql, 3), expected);
        assert_eq!(normalize_postgres_statement(sql, 1), sql);
    }

    #[test]
    fn malformed_indices_and_unterminated_quotes_do_not_panic() {
        assert_eq!(normalize_postgres_statement("SELECT $0", 3), "SELECT $0");
        for sql in [
            "SELECT $99999999999999999999999999999999999999",
            "SELECT $",
            "SELECT $tag$",
            "SELECT '$1",
            "SELECT E'\\",
            "SELECT /* $1",
            "SELECT $0",
        ] {
            let _ = normalize_postgres_statement(sql, usize::MAX);
        }
    }
}

/// Words that cannot appear as a bare table or column name in a statement the
/// ORM generates on SQLite 3.46: CREATE/ALTER/DROP TABLE, column definitions,
/// key and unique lists, foreign keys, indexes, partial-index predicates,
/// CHECK expressions and DML. Measured against the engine, not copied from its
/// keyword list: most SQLite keywords are accepted as identifiers.
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

/// PostgreSQL 17 reserved keywords and keywords reserved as names but allowed
/// as functions or types (`pg_get_keywords()` categories `R` and `T`). They are
/// exactly the keywords the statements described above reject as bare names;
/// unreserved and column-name keywords are accepted.
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

/// Transact-SQL reserved keywords (SQL Server documentation).
const MSSQL_RESERVED_IDENTIFIERS: &[&str] = &[
    "add",
    "all",
    "alter",
    "and",
    "any",
    "as",
    "asc",
    "authorization",
    "backup",
    "begin",
    "between",
    "break",
    "browse",
    "bulk",
    "by",
    "cascade",
    "case",
    "check",
    "checkpoint",
    "close",
    "clustered",
    "coalesce",
    "collate",
    "column",
    "commit",
    "compute",
    "constraint",
    "contains",
    "containstable",
    "continue",
    "convert",
    "create",
    "cross",
    "current",
    "current_date",
    "current_time",
    "current_timestamp",
    "current_user",
    "cursor",
    "database",
    "dbcc",
    "deallocate",
    "declare",
    "default",
    "delete",
    "deny",
    "desc",
    "disk",
    "distinct",
    "distributed",
    "double",
    "drop",
    "dump",
    "else",
    "end",
    "errlvl",
    "escape",
    "except",
    "exec",
    "execute",
    "exists",
    "exit",
    "external",
    "fetch",
    "file",
    "fillfactor",
    "for",
    "foreign",
    "freetext",
    "freetexttable",
    "from",
    "full",
    "function",
    "goto",
    "grant",
    "group",
    "having",
    "holdlock",
    "identity",
    "identity_insert",
    "identitycol",
    "if",
    "in",
    "index",
    "inner",
    "insert",
    "intersect",
    "into",
    "is",
    "join",
    "key",
    "kill",
    "left",
    "like",
    "lineno",
    "load",
    "merge",
    "national",
    "nocheck",
    "nonclustered",
    "not",
    "null",
    "nullif",
    "of",
    "off",
    "offsets",
    "on",
    "open",
    "opendatasource",
    "openquery",
    "openrowset",
    "openxml",
    "option",
    "or",
    "order",
    "outer",
    "over",
    "percent",
    "pivot",
    "plan",
    "precision",
    "primary",
    "print",
    "proc",
    "procedure",
    "public",
    "raiserror",
    "read",
    "readtext",
    "reconfigure",
    "references",
    "replication",
    "restore",
    "restrict",
    "return",
    "revert",
    "revoke",
    "right",
    "rollback",
    "rowcount",
    "rowguidcol",
    "rule",
    "save",
    "schema",
    "securityaudit",
    "select",
    "semantickeyphrasetable",
    "semanticsimilaritydetailstable",
    "semanticsimilaritytable",
    "session_user",
    "set",
    "setuser",
    "shutdown",
    "some",
    "statistics",
    "system_user",
    "table",
    "tablesample",
    "textsize",
    "then",
    "to",
    "top",
    "tran",
    "transaction",
    "trigger",
    "truncate",
    "try_convert",
    "tsequal",
    "union",
    "unique",
    "unpivot",
    "update",
    "updatetext",
    "use",
    "user",
    "values",
    "varying",
    "view",
    "waitfor",
    "when",
    "where",
    "while",
    "with",
    "within",
    "writetext",
];

// SQL identifier delimiters may contain dots and doubled closing delimiters.
// Slice only at dots outside a delimited part, reading each part with the
// backend's own delimited forms (`DatabaseBackend::identifier_delimiters`):
// PostgreSQL has only `"..."`, SQLite closes `[...]` at the first `]`, and
// SQL Server has no backtick form. Byte offsets are safe because every
// delimiter is ASCII.
fn identifier_path_parts(backend: DatabaseBackend, identifier: &str) -> impl Iterator<Item = &str> {
    let delimiters = backend.identifier_delimiters();
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

/// The names PostgreSQL resolves each part of a declared path to when the
/// ORM's SQL renders it with [`DatabaseBackend::render_identifier_path`]:
/// a delimited part keeps its case, a part rendered bare folds to lower case,
/// and a part that rendering quotes keeps its case.
pub(crate) fn postgres_resolved_identifier_parts(path: &str) -> Vec<String> {
    identifier_path_parts(DatabaseBackend::Postgres, path)
        .filter(|part| !part.is_empty())
        .map(postgres_resolved_identifier)
        .collect()
}

/// The name PostgreSQL resolves one identifier to when the ORM's SQL renders
/// it with [`DatabaseBackend::render_identifier`].
///
/// Text that starts with `"` but is not exactly one well-formed quoted
/// identifier names itself in full, as policy SQL quotes it.
pub(crate) fn postgres_resolved_identifier(identifier: &str) -> String {
    if let Some(name) = postgres_delimited_identifier(identifier) {
        name
    } else if identifier.starts_with('"')
        || DatabaseBackend::Postgres.identifier_needs_quoting(identifier)
    {
        identifier.to_string()
    } else {
        identifier.to_ascii_lowercase()
    }
}

/// The name inside `identifier` when it is exactly one well-formed quoted
/// PostgreSQL identifier: it starts and ends with `"`, is not empty inside,
/// contains no NUL, and every inner `"` is doubled.
pub(crate) fn postgres_delimited_identifier(identifier: &str) -> Option<String> {
    let inner = identifier.strip_prefix('"')?.strip_suffix('"')?;
    let mut name = String::with_capacity(inner.len());
    let mut characters = inner.chars();
    while let Some(character) = characters.next() {
        if character == '"' && characters.next() != Some('"') {
            return None;
        }
        name.push(character);
    }
    (!name.is_empty() && !name.contains('\0')).then_some(name)
}

/// Whether `character` is a Basic Multilingual Plane character in one of the
/// generated SQL Server regular-identifier tables.
fn mssql_identifier_character(character: char, table: &[(u16, u16)]) -> bool {
    u16::try_from(u32::from(character)).is_ok_and(|unit| {
        table
            .binary_search_by(|&(low, high)| {
                if high < unit {
                    std::cmp::Ordering::Less
                } else if low > unit {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .is_ok()
    })
}

/// A raw PostgreSQL catalogue relation name in the declared-name form
/// introspection reports: unchanged when the ORM's renderers resolve that
/// text back to the same relation, and otherwise delimited literally. A name
/// that bare SQL folds (`Archive`), that starts with `"` (delimited or not,
/// such as an unmatched `"archive`), or that the path splitter would split at
/// a dot (`local.parent`) is delimited; an ordinary or reserved name keeps its
/// text.
#[cfg(feature = "postgres")]
pub(crate) fn postgres_reported_relation_name(name: &str) -> String {
    if postgres_resolved_identifier(name) != name
        || name.starts_with('"')
        || identifier_path_parts(DatabaseBackend::Postgres, name)
            .nth(1)
            .is_some()
    {
        DatabaseBackend::Postgres.delimit_raw_identifier(name)
    } else {
        name.to_owned()
    }
}

/// Whether a declared table path has a single part, with no schema, as
/// `backend` splits it.
pub(crate) fn identifier_path_is_bare(backend: DatabaseBackend, path: &str) -> bool {
    identifier_path_parts(backend, path)
        .filter(|part| !part.is_empty())
        .count()
        == 1
}

impl DatabaseBackend {
    fn reserved_identifiers(self) -> &'static [&'static str] {
        match self {
            Self::Sqlite => SQLITE_RESERVED_IDENTIFIERS,
            Self::Postgres => POSTGRES_RESERVED_IDENTIFIERS,
            Self::Mssql => MSSQL_RESERVED_IDENTIFIERS,
            Self::Mysql => &[],
        }
    }

    /// Opening delimiter, closing delimiter, and whether a doubled closing
    /// delimiter stands for one inside the name, for each delimited form this
    /// backend's parser reads.
    fn identifier_delimiters(self) -> &'static [(char, char, bool)] {
        match self {
            // SQLite reads `[...]` up to the first `]`, with no escape.
            Self::Sqlite => &[('"', '"', true), ('`', '`', true), ('[', ']', false)],
            Self::Postgres => &[('"', '"', true)],
            // `"..."` assumes `QUOTED_IDENTIFIER ON`, the driver default.
            Self::Mssql => &[('[', ']', true), ('"', '"', true)],
            Self::Mysql => &[],
        }
    }

    /// Whether `identifier` is exactly one well-formed delimited identifier
    /// for this backend: it opens with a delimiter, every closing delimiter
    /// inside it is doubled where the form allows that (SQLite's `[...]`
    /// allows no `]` inside), its only undoubled closing delimiter is its last
    /// character, and it contains no NUL. The name inside may be empty only on
    /// SQLite, which accepts `""`, `[]` and two backticks as a name;
    /// PostgreSQL and SQL Server refuse an empty delimited identifier.
    ///
    /// This is the only input [`SqlDialect::quote_identifier`] returns
    /// unchanged. Text that merely starts and ends with delimiter characters,
    /// such as `"a"b"` or `[a] , [b]`, is not one identifier (#782).
    fn is_one_delimited_identifier(self, identifier: &str) -> bool {
        if identifier.contains('\0') {
            return false;
        }
        let mut characters = identifier.chars();
        let Some(open) = characters.next() else {
            return false;
        };
        let Some(&(_, close, doubled)) = self
            .identifier_delimiters()
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
            return (!empty || self == Self::Sqlite) && characters.next().is_none();
        }
        false
    }

    /// `name` delimited as exactly one identifier that names `name` itself,
    /// even when `name` already looks delimited. For raw names read back from
    /// a database catalog, which are names rather than SQL text: a table
    /// whose name is literally `"x"` must not be addressed as `x`. MySQL
    /// identifiers are returned unchanged.
    pub(crate) fn delimit_raw_identifier(self, name: &str) -> String {
        match self {
            Self::Mssql => format!("[{}]", name.replace(']', "]]")),
            Self::Postgres | Self::Sqlite => format!("\"{}\"", name.replace('"', "\"\"")),
            Self::Mysql => name.to_string(),
        }
    }

    /// A raw catalogue name in the declared-name convention that the
    /// migration renderers read: unchanged when quoting it would name it,
    /// and otherwise delimited literally. A name that is itself one
    /// well-formed delimited identifier (such as an index literally named
    /// `[ix]`) or that the backend's path splitter would split (such as
    /// `a.b`) is delimited, so re-rendering it names the catalogue object
    /// rather than `ix` or `b`. Every ordinary name keeps its text, so
    /// introspected models and migration plans are unchanged for them.
    #[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
    pub(crate) fn raw_name_in_declared_form(self, name: &str) -> String {
        if self.is_one_delimited_identifier(name) || name.contains('.') {
            self.delimit_raw_identifier(name)
        } else {
            name.to_owned()
        }
    }

    /// Whether a declared column path's qualifier, if any, names `table`, the
    /// declaring entity's own table path. An unqualified column always
    /// does. A qualified column (`"t"."c"`, `main.t.c`) does only when its
    /// qualifier equals the trailing parts of `table`, compared as the
    /// backend compares identifiers: PostgreSQL by resolved name, SQLite
    /// without ASCII case, SQL Server and others exactly. Read predicates
    /// refuse any other qualifier, because it could bind the column to
    /// another table in a correlated subquery (#782).
    pub(crate) fn column_qualifier_names_table(self, column: &str, table: &str) -> bool {
        let parts = identifier_path_parts(self, column)
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        let Some((_, qualifier)) = parts.split_last() else {
            return false;
        };
        let table = identifier_path_parts(self, table)
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        qualifier.len() <= table.len()
            && qualifier
                .iter()
                .rev()
                .zip(table.iter().rev())
                .all(|(left, right)| self.same_identifier_part(left, right))
    }

    /// Whether two path parts name the same identifier on this backend.
    pub(super) fn same_identifier_part(self, left: &str, right: &str) -> bool {
        match self {
            Self::Postgres => {
                postgres_resolved_identifier(left) == postgres_resolved_identifier(right)
            }
            Self::Sqlite => self
                .identifier_part_name(left)
                .eq_ignore_ascii_case(&self.identifier_part_name(right)),
            Self::Mssql | Self::Mysql => {
                self.identifier_part_name(left) == self.identifier_part_name(right)
            }
        }
    }

    pub(super) fn mssql_schema_and_table(self, path: &str) -> Option<(String, String)> {
        debug_assert_eq!(self, Self::Mssql);
        let parts = identifier_path_parts(self, path).collect::<Vec<_>>();
        let (schema, table) = match parts.as_slice() {
            [table] => ("dbo", *table),
            [schema, table] => (*schema, *table),
            _ => return None,
        };
        if schema.is_empty() || table.is_empty() {
            return None;
        }
        Some((
            self.identifier_part_name(schema),
            self.identifier_part_name(table),
        ))
    }

    /// The name a table path's last part denotes, undelimited.
    pub(crate) fn table_name_part(self, table: &str) -> String {
        let part = identifier_path_parts(self, table)
            .filter(|part| !part.is_empty())
            .last()
            .unwrap_or(table);
        self.identifier_part_name(part)
    }

    /// The name one path part denotes: the inside of a well-formed delimited
    /// identifier, or the part itself.
    pub(super) fn delimited_column_name(self, part: &str) -> Option<String> {
        self.is_one_delimited_identifier(part)
            .then(|| self.identifier_part_name(part))
    }

    pub(crate) fn identifier_part_name(self, part: &str) -> String {
        if !self.is_one_delimited_identifier(part) {
            return part.to_owned();
        }
        let mut characters = part.chars();
        let open = characters.next().unwrap_or_default();
        let (_, close, doubled) = self
            .identifier_delimiters()
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

    /// The column a declared column path names, as one delimited identifier
    /// to follow a table or alias prefix (`alias.<column>`) or to stand
    /// unqualified in a predicate.
    ///
    /// A column reference can be table-qualified, such as `"t"."c"`.
    /// The dialect splits the path and delimits its last part with
    /// [`SqlDialect::quote_identifier`]. In a derived entity, `db_column`
    /// declares a single physical column.
    /// Use this wherever the column is referenced through an alias or inside
    /// a subquery: the whole path would read as `schema.table.column` after a
    /// prefix, and a literal-quoted path can read as a string constant on
    /// SQLite.
    ///
    /// ```
    /// use graphql_orm::graphql::orm::DatabaseBackend;
    ///
    /// assert_eq!(DatabaseBackend::Sqlite.quote_column_identifier("owner"), "\"owner\"");
    /// assert_eq!(DatabaseBackend::Sqlite.quote_column_identifier("\"t\".\"c\""), "\"c\"");
    /// assert_eq!(DatabaseBackend::Mssql.quote_column_identifier("[t].[a.b]"), "[a.b]");
    /// ```
    pub fn quote_column_identifier(self, column: &str) -> String {
        let part = identifier_path_parts(self, column)
            .filter(|part| !part.is_empty())
            .last()
            .unwrap_or(column);
        self.quote_identifier(part)
    }

    /// Generated filter support: a rendered filter column of the row
    /// `row`, as `row.<column>`, the column written with
    /// [`Self::delimited_column_identifier`] (#796).
    #[doc(hidden)]
    pub fn row_bound_filter_column(self, row: &str, column: &str) -> String {
        format!("{row}.{}", self.delimited_column_identifier(column))
    }

    /// The column a declared or rendered column path names, as one
    /// delimited identifier that addresses exactly the column generated DDL
    /// and DML address.
    ///
    /// Generated SQL writes a plain, non-reserved column bare, which
    /// PostgreSQL folds to lower case; such a part is folded before it is
    /// delimited, as [`Self::render_table_identifier_path`] does for
    /// tables. An explicitly delimited or non-plain part keeps its case.
    /// SQLite and SQL Server delimit the part as it is.
    ///
    /// ```
    /// use graphql_orm::graphql::orm::DatabaseBackend;
    ///
    /// assert_eq!(DatabaseBackend::Sqlite.delimited_column_identifier("OwnerId"), "\"OwnerId\"");
    /// assert_eq!(DatabaseBackend::Postgres.delimited_column_identifier("OwnerId"), "\"ownerid\"");
    /// assert_eq!(DatabaseBackend::Postgres.delimited_column_identifier("\"OwnerId\""), "\"OwnerId\"");
    /// assert_eq!(DatabaseBackend::Mssql.delimited_column_identifier("t.owner"), "[owner]");
    /// ```
    pub fn delimited_column_identifier(self, column: &str) -> String {
        let part = identifier_path_parts(self, column)
            .filter(|part| !part.is_empty())
            .last()
            .unwrap_or(column);
        if self == Self::Postgres && !part.starts_with('"') && !self.identifier_needs_quoting(part)
        {
            self.quote_identifier(&part.to_ascii_lowercase())
        } else {
            self.quote_identifier(part)
        }
    }

    fn identifier_needs_quoting(self, identifier: &str) -> bool {
        if self == Self::Mysql || self.is_one_delimited_identifier(identifier) {
            // MySQL identifiers are rendered verbatim, and an identifier that is
            // already one well-formed delimited identifier is passed through.
            return false;
        }
        let mut characters = identifier.chars();
        let Some(first) = characters.next() else {
            return true;
        };
        // Non-ASCII characters are identifier characters on SQLite and
        // PostgreSQL and stay bare, so their case folding is unchanged. SQL
        // Server reads a name bare only under its regular-identifier rule,
        // the Unicode 3.2 identifier classes of the Basic Multilingual Plane
        // (#783); see `mssql_identifier_tables`. MSSQL `#`/`@` prefixes keep
        // their temporary-table and variable meaning.
        let plain_start = |character: char| {
            character.is_ascii_alphabetic()
                || character == '_'
                || match self {
                    Self::Mssql => {
                        matches!(character, '#' | '@')
                            || mssql_identifier_character(character, MSSQL_IDENTIFIER_START)
                    }
                    _ => !character.is_ascii(),
                }
        };
        let plain_rest = |character: char| {
            plain_start(character)
                || character.is_ascii_digit()
                || character == '$'
                || (self == Self::Mssql
                    && mssql_identifier_character(character, MSSQL_IDENTIFIER_PART))
        };
        // SQLite reads a byte-order mark that starts a token as whitespace,
        // so a bare name beginning with one would lose it (#725).
        if !plain_start(first)
            || (self == Self::Sqlite && first == '\u{feff}')
            || !characters.all(plain_rest)
        {
            return true;
        }
        let lowercase = identifier.to_ascii_lowercase();
        self.reserved_identifiers()
            .binary_search(&lowercase.as_str())
            .is_ok()
    }

    /// One identifier as generated DDL and DML write it.
    ///
    /// A plain identifier (an ASCII letter, `_` or non-ASCII character, then
    /// those, digits or `$`) that is not a reserved word on this backend is
    /// written bare, exactly as earlier releases wrote it, so statement text,
    /// plan hashes and PostgreSQL case folding are unchanged for every name
    /// that was already accepted. On SQL Server a non-ASCII character is plain
    /// only when the server reads it in a regular identifier: a Unicode 3.2
    /// letter or letter number (or, after the first character, a mark,
    /// decimal digit, connector or format character) in the Basic
    /// Multilingual Plane, so names such as `x²` or `x🦀` are bracketed.
    /// SQL Server runtime rendering preserves bare `#` and `@` prefixes for
    /// temporary tables and variables. The derive delimits declared names.
    /// This renderer is infallible, so SQL Server enforces its name length limit
    /// when executing the statement.
    /// SQLite never writes a name bare that starts with a byte-order mark. Reserved words such as `order`, `join` or `select`,
    /// and names with other characters, are delimited with
    /// [`SqlDialect::quote_identifier`]; input that is already exactly one
    /// well-formed delimited identifier is returned unchanged. MySQL
    /// identifiers are returned unchanged.
    ///
    /// ```
    /// use graphql_orm::graphql::orm::DatabaseBackend;
    ///
    /// assert_eq!(DatabaseBackend::Sqlite.render_identifier("link_table"), "link_table");
    /// assert_eq!(DatabaseBackend::Sqlite.render_identifier("join"), "\"join\"");
    /// assert_eq!(DatabaseBackend::Postgres.render_identifier_path("app.order"), "app.\"order\"");
    /// ```
    pub fn render_identifier(self, identifier: &str) -> std::borrow::Cow<'_, str> {
        if self.identifier_needs_quoting(identifier) {
            std::borrow::Cow::Owned(self.quote_identifier(identifier))
        } else {
            std::borrow::Cow::Borrowed(identifier)
        }
    }

    /// [`Self::render_identifier`] for each part of a qualified name.
    ///
    /// Dots inside quoted identifiers and doubled closing delimiters are preserved.
    pub fn render_identifier_path(self, identifier: &str) -> std::borrow::Cow<'_, str> {
        if !identifier_path_parts(self, identifier)
            .any(|part| !part.is_empty() && self.identifier_needs_quoting(part))
        {
            return std::borrow::Cow::Borrowed(identifier);
        }
        std::borrow::Cow::Owned(
            identifier_path_parts(self, identifier)
                .filter(|part| !part.is_empty())
                .map(|part| self.render_identifier(part))
                .collect::<Vec<_>>()
                .join("."),
        )
    }

    /// Delimit a table path while preserving the resolution of generated DDL.
    ///
    /// PostgreSQL historically creates plain, non-reserved table components
    /// bare. Fold those components before delimiting them, so a quoted DML or
    /// index reference still names that same table. Explicitly delimited and
    /// non-plain components retain their case. Other backends keep their
    /// existing quoted rendering. Declared metadata and helper names are unchanged.
    pub fn render_table_identifier_path(self, identifier: &str) -> String {
        if self != Self::Postgres {
            return self.quote_identifier_path(identifier);
        }
        identifier_path_parts(self, identifier)
            .filter(|part| !part.is_empty())
            .map(|part| {
                if !part.starts_with('"') && !self.identifier_needs_quoting(part) {
                    self.quote_identifier(&part.to_ascii_lowercase())
                } else {
                    self.quote_identifier(part)
                }
            })
            .collect::<Vec<_>>()
            .join(".")
    }

    /// One string constant as generated DDL writes it, or `None` for MySQL,
    /// which has no executable lane and whose quoting depends on its SQL mode.
    ///
    /// Quotes are doubled on every backend. A PostgreSQL value that contains a
    /// backslash is written as an escape string (`E'…'`) with doubled
    /// backslashes, which the server reads the same way under either
    /// `standard_conforming_strings` setting; other values keep the plain
    /// `'…'` spelling of earlier releases. SQL Server values are Unicode
    /// (`N'…'`), and a backslash before a line break is split into a separate
    /// concatenated constant so it is not read as a line continuation.
    pub(crate) fn render_string_literal(self, value: &str) -> Option<String> {
        let quoted = |value: &str| value.replace('\'', "''");
        Some(match self {
            Self::Sqlite => format!("'{}'", quoted(value)),
            Self::Postgres if value.contains('\\') => {
                format!("E'{}'", quoted(&value.replace('\\', "\\\\")))
            }
            Self::Postgres => format!("'{}'", quoted(value)),
            Self::Mysql => return None,
            Self::Mssql => {
                let mut parts = Vec::new();
                let mut rest = value;
                while let Some(index) = rest
                    .find("\\\n")
                    .into_iter()
                    .chain(rest.find("\\\r\n"))
                    .min()
                {
                    parts.push(format!("N'{}'", quoted(&rest[..=index])));
                    rest = &rest[index + 1..];
                }
                parts.push(format!("N'{}'", quoted(rest)));
                if parts.len() == 1 {
                    parts.remove(0)
                } else {
                    format!("({})", parts.join(" + "))
                }
            }
        })
    }

    /// [`Self::render_identifier`] for each column of a list, joined by `, `.
    pub(crate) fn render_identifier_list<'a>(
        self,
        identifiers: impl IntoIterator<Item = &'a str>,
    ) -> String {
        identifiers
            .into_iter()
            .map(|identifier| self.render_identifier(identifier))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[cfg(test)]
mod identifier_rendering_tests {
    use super::*;

    #[test]
    fn string_literals_quote_for_each_dialect() {
        use DatabaseBackend as B;
        for (backend, value, expected) in [
            (B::Sqlite, "it's", "'it''s'"),
            (B::Sqlite, r"a\'b", r"'a\''b'"),
            (B::Postgres, "it's", "'it''s'"),
            (B::Postgres, "", "''"),
            (B::Postgres, r"a\'b\", r"E'a\\''b\\'"),
            (B::Postgres, r"\x00ff", r"E'\\x00ff'"),
            (B::Mssql, "it's 雪", "N'it''s 雪'"),
            (B::Mssql, "a\\\nb", "(N'a\\' + N'\nb')"),
            (B::Mssql, "a\\\r\nb\\\nc", "(N'a\\' + N'\r\nb\\' + N'\nc')"),
            (B::Mssql, "a\\b\n", "N'a\\b\n'"),
        ] {
            assert_eq!(
                backend.render_string_literal(value).as_deref(),
                Some(expected),
                "{backend:?} {value:?}"
            );
        }
        assert_eq!(B::Mysql.render_string_literal("plain"), None);
    }

    #[test]
    fn reserved_word_lists_are_sorted_lowercase_and_unique() {
        for list in [
            SQLITE_RESERVED_IDENTIFIERS,
            POSTGRES_RESERVED_IDENTIFIERS,
            MSSQL_RESERVED_IDENTIFIERS,
        ] {
            assert!(list.windows(2).all(|pair| pair[0] < pair[1]), "{list:?}");
            assert!(list.iter().all(|word| *word == word.to_ascii_lowercase()));
        }
        assert_eq!(SQLITE_RESERVED_IDENTIFIERS.len(), 64);
        assert_eq!(POSTGRES_RESERVED_IDENTIFIERS.len(), 101);
    }

    #[test]
    fn accepted_identifiers_render_byte_identically() {
        for backend in [
            DatabaseBackend::Sqlite,
            DatabaseBackend::Postgres,
            DatabaseBackend::Mssql,
            DatabaseBackend::Mysql,
        ] {
            for identifier in [
                "users",
                "link_table",
                "createdAt",
                "_private",
                "col$1",
                "名前",
            ] {
                assert_eq!(backend.render_identifier(identifier), identifier);
            }
            assert_eq!(backend.render_identifier_path("app.users"), "app.users");
        }
        // Keywords each engine accepts as names stay bare.
        assert_eq!(DatabaseBackend::Sqlite.render_identifier("key"), "key");
        assert_eq!(
            DatabaseBackend::Sqlite.render_identifier("action"),
            "action"
        );
        assert_eq!(DatabaseBackend::Postgres.render_identifier("key"), "key");
        assert_eq!(
            DatabaseBackend::Postgres.render_identifier("position"),
            "position"
        );
        assert_eq!(
            DatabaseBackend::Mssql.render_identifier("#scratch"),
            "#scratch"
        );
    }

    #[test]
    fn reserved_and_irregular_identifiers_are_delimited() {
        let sqlite = DatabaseBackend::Sqlite;
        let postgres = DatabaseBackend::Postgres;
        let mssql = DatabaseBackend::Mssql;
        assert_eq!(sqlite.render_identifier("join"), "\"join\"");
        assert_eq!(sqlite.render_identifier("ORDER"), "\"ORDER\"");
        assert_eq!(postgres.render_identifier("select"), "\"select\"");
        assert_eq!(postgres.render_identifier("user"), "\"user\"");
        assert_eq!(mssql.render_identifier("key"), "[key]");
        assert_eq!(
            mssql.render_identifier_path("public.users"),
            "[public].users"
        );
        assert_eq!(sqlite.render_identifier("1st"), "\"1st\"");
        assert_eq!(sqlite.render_identifier("two words"), "\"two words\"");
        assert_eq!(sqlite.render_identifier("a\"b"), "\"a\"\"b\"");
        assert_eq!(sqlite.render_identifier(""), "\"\"");
        assert_eq!(
            postgres.render_identifier_path("app.order"),
            "app.\"order\""
        );
        assert_eq!(
            sqlite.render_identifier_list(["id", "join", "select"]),
            "id, \"join\", \"select\""
        );
        // One well-formed delimited identifier is not delimited twice; text
        // that only starts and ends with delimiters is one literal name.
        assert_eq!(sqlite.render_identifier("\"join\""), "\"join\"");
        assert_eq!(mssql.render_identifier("[key]"), "[key]");
        assert_eq!(sqlite.render_identifier("\"a\"b\""), "\"\"\"a\"\"b\"\"\"");
        assert_eq!(mssql.render_identifier("[a] , [b]"), "[[a]] , [b]]]");
        assert_eq!(DatabaseBackend::Mysql.render_identifier("order"), "order");
    }

    #[test]
    fn column_qualifiers_must_name_the_entitys_own_table() {
        use DatabaseBackend::{Mssql, Postgres, Sqlite};
        for (backend, column, table, names) in [
            (Sqlite, "owner", "main.parents", true),
            (Sqlite, "\"parents\".\"owner\"", "main.parents", true),
            (Sqlite, "PARENTS.owner", "main.parents", true),
            (Sqlite, "main.parents.owner", "main.parents", true),
            (Sqlite, "[main].\"parents\".owner", "main.parents", true),
            (Sqlite, "\"children\".\"owner\"", "main.parents", false),
            (Sqlite, "other.parents.owner", "main.parents", false),
            (Sqlite, "main.parents.owner", "parents", false),
            (Postgres, "\"parents\".owner", "public.parents", true),
            (Postgres, "PARENTS.owner", "public.parents", true),
            (Postgres, "\"PARENTS\".owner", "public.parents", false),
            (Postgres, "\"Parents\".owner", "\"Parents\"", true),
            (Postgres, "parents.owner", "\"Parents\"", false),
            (
                Postgres,
                "\"other\".\"parents\".owner",
                "public.parents",
                false,
            ),
            (Mssql, "[parents].[owner]", "dbo.parents", true),
            (Mssql, "\"parents\".owner", "[dbo].[parents]", true),
            (Mssql, "[Parents].[owner]", "dbo.parents", false),
            (Mssql, "[a]].b].[owner]", "dbo.[a]].b]", true),
            (Mssql, "[children].[owner]", "dbo.parents", false),
        ] {
            assert_eq!(
                backend.column_qualifier_names_table(column, table),
                names,
                "{backend:?} {column} {table}"
            );
        }
    }

    #[test]
    fn raw_catalog_names_are_always_delimited_literally() {
        assert_eq!(
            DatabaseBackend::Sqlite.delimit_raw_identifier("users"),
            "\"users\""
        );
        assert_eq!(
            DatabaseBackend::Sqlite.delimit_raw_identifier("\"users\""),
            "\"\"\"users\"\"\""
        );
        assert_eq!(
            DatabaseBackend::Postgres.delimit_raw_identifier("a\"b"),
            "\"a\"\"b\""
        );
        assert_eq!(
            DatabaseBackend::Mssql.delimit_raw_identifier("[x]"),
            "[[x]]]"
        );
        // Quoting a declared identifier passes one well-formed token through.
        assert_eq!(
            DatabaseBackend::Sqlite.quote_identifier("\"users\""),
            "\"users\""
        );
    }

    #[test]
    fn macro_reserved_identifier_lists_match_the_runtime() {
        // The derive crate cannot depend on this crate, so it carries copies of
        // the SQLite and PostgreSQL lists. The derive always delimits SQL
        // Server names and therefore needs no reserved list for that backend.
        // Compare the shared lists when the workspace sources are present.
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../graphql-orm-macros/src/backend.rs"
        );
        let Ok(source) = std::fs::read_to_string(path) else {
            return;
        };
        let words = |name: &str| -> Vec<String> {
            let start = source
                .find(&format!("const {name}: &[&str] = &["))
                .expect("list should exist in the derive crate");
            let body = &source[start..];
            let body = &body[body.find("&[").unwrap() + 2..body.find("];").unwrap()];
            body.split('"')
                .skip(1)
                .step_by(2)
                .map(str::to_string)
                .collect()
        };
        assert_eq!(
            words("SQLITE_RESERVED_IDENTIFIERS"),
            SQLITE_RESERVED_IDENTIFIERS
        );
        assert_eq!(
            words("POSTGRES_RESERVED_IDENTIFIERS"),
            POSTGRES_RESERVED_IDENTIFIERS
        );
    }

    #[test]
    fn quoted_identifier_paths_match_the_macro_cases() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../graphql-orm-macros/fixtures/identifier-paths.tsv"
        );
        let Ok(cases) = std::fs::read_to_string(path) else {
            return;
        };
        for line in cases.lines() {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let mut columns = line.split('\t');
            let backend = match columns.next().expect("backend") {
                "sqlite" => DatabaseBackend::Sqlite,
                "postgres" => DatabaseBackend::Postgres,
                "mssql" => DatabaseBackend::Mssql,
                other => panic!("unknown backend {other}"),
            };
            let input = columns.next().expect("input");
            let expected = columns.next().expect("expected");
            assert_eq!(backend.render_identifier_path(input), expected, "{line}");
            let quoted = backend.quote_identifier_path(input);
            assert_eq!(backend.quote_identifier_path(&quoted), quoted, "{line}");
        }
    }
}

pub trait SqlDialect {
    fn backend(&self) -> DatabaseBackend;
    /// One identifier, always written as exactly one delimited identifier.
    ///
    /// Input that is already exactly one well-formed delimited identifier for
    /// the dialect is returned unchanged, so quoting is idempotent. It may not
    /// contain NUL, and its name may be empty only on SQLite. Any other input, including text that only starts and ends
    /// with delimiter characters such as `"a"b"` or `[a] , [b]`, is delimited
    /// as a literal name with the closing delimiter doubled, so the result
    /// always reads back as one name. SQL Server uses `[...]`; SQLite and
    /// PostgreSQL use `"..."`. MySQL identifiers are returned unchanged.
    ///
    /// ```
    /// use graphql_orm::graphql::orm::{DatabaseBackend, SqlDialect};
    ///
    /// assert_eq!(DatabaseBackend::Postgres.quote_identifier("order"), "\"order\"");
    /// assert_eq!(DatabaseBackend::Postgres.quote_identifier("\"order\""), "\"order\"");
    /// assert_eq!(DatabaseBackend::Postgres.quote_identifier("\"a\"b\""), "\"\"\"a\"\"b\"\"\"");
    /// assert_eq!(DatabaseBackend::Mssql.quote_identifier("[a] , [b]"), "[[a]] , [b]]]");
    /// ```
    fn quote_identifier(&self, identifier: &str) -> String;
    /// [`Self::quote_identifier`] for each part of a qualified name, split at
    /// dots outside delimited parts as the dialect reads them: `"..."` on
    /// PostgreSQL; `"..."`, backticks and `[...]` (closed at the first `]`) on
    /// SQLite; `[...]` and `"..."` on SQL Server.
    fn quote_identifier_path(&self, identifier: &str) -> String;
    fn placeholder(&self, index: usize) -> String;
    /// Rebase fragment placeholders by occurrence, starting at `start_index`.
    ///
    /// Native labels in fragments are not binding identities: generated nested
    /// fragments may reuse labels with separate values. Complete statements with
    /// native PostgreSQL identities use [`super::backend::OrmBackend::normalize_sql`]
    /// at the driver boundary instead. Anonymous/mixed rebasing remains unchanged.
    fn normalize_sql(&self, sql: &str, start_index: usize) -> String;
    fn count_projection(&self) -> &'static str;
    fn render_pagination(&self, limit: Option<i64>, offset: i64) -> String;
    fn relation_key_cast(&self, column: &str) -> String;
    /// Largest number of bound values one batched statement should carry.
    ///
    /// SQL Server accepts 2,100 parameters per request and the driver's
    /// `sp_executesql` call uses two of them. PostgreSQL's wire protocol
    /// counts parameters in 16 bits and SQLite's default limit is 32,766.
    fn max_statement_binds(&self) -> usize {
        match self.backend() {
            DatabaseBackend::Mssql => 2_000,
            DatabaseBackend::Postgres | DatabaseBackend::Sqlite | DatabaseBackend::Mysql => 32_000,
        }
    }
    fn current_epoch_expr(&self) -> &'static str;
    fn current_date_expr(&self) -> &'static str;
    fn ci_like(&self, column: &str, placeholder: &str) -> String;
    fn days_ago_expr(&self, days: i64) -> String;
    fn days_ahead_expr(&self, days: i64) -> String;
    fn spatial_geojson_expr(&self, placeholder: &str, srid: i32) -> String;
    fn spatial_predicate(
        &self,
        predicate: SpatialPredicate,
        column: &str,
        geometry_expr: &str,
    ) -> String;
}

impl SqlDialect for DatabaseBackend {
    fn backend(&self) -> DatabaseBackend {
        *self
    }

    fn quote_identifier(&self, identifier: &str) -> String {
        match self {
            DatabaseBackend::Mysql => identifier.to_string(),
            _ if self.is_one_delimited_identifier(identifier) => identifier.to_string(),
            _ => self.delimit_raw_identifier(identifier),
        }
    }

    fn quote_identifier_path(&self, identifier: &str) -> String {
        if *self != DatabaseBackend::Mssql
            && *self != DatabaseBackend::Postgres
            && *self != DatabaseBackend::Sqlite
        {
            return identifier.to_string();
        }

        identifier_path_parts(*self, identifier)
            .filter(|part| !part.is_empty())
            .map(|part| self.quote_identifier(part))
            .collect::<Vec<_>>()
            .join(".")
    }

    fn placeholder(&self, index: usize) -> String {
        match self {
            DatabaseBackend::Postgres => format!("${index}"),
            DatabaseBackend::Mssql => format!("@P{index}"),
            DatabaseBackend::Sqlite | DatabaseBackend::Mysql => "?".to_string(),
        }
    }

    fn normalize_sql(&self, sql: &str, start_index: usize) -> String {
        normalize_sql_placeholders(*self, sql, start_index)
    }

    fn count_projection(&self) -> &'static str {
        match self {
            DatabaseBackend::Mssql => "COUNT_BIG(*) AS [count]",
            _ => "COUNT(*) AS count",
        }
    }

    fn render_pagination(&self, limit: Option<i64>, offset: i64) -> String {
        match self {
            DatabaseBackend::Mssql => match limit {
                // SQL Server rejects `FETCH NEXT 0 ROWS`. The select renderer
                // serves an empty page with `TOP (0)` and never reaches this
                // arm; for any other caller an offset past every row is the
                // one clause that returns no rows.
                Some(limit) if limit <= 0 => {
                    format!(" OFFSET {} ROWS FETCH NEXT 1 ROWS ONLY", i64::MAX)
                }
                Some(limit) => {
                    format!(
                        " OFFSET {} ROWS FETCH NEXT {} ROWS ONLY",
                        offset.max(0),
                        limit
                    )
                }
                None if offset > 0 => format!(" OFFSET {} ROWS", offset),
                None => String::new(),
            },
            _ => {
                let mut sql = String::new();
                match limit {
                    Some(limit) => sql.push_str(&format!(" LIMIT {}", limit.max(0))),
                    // SQLite and MySQL accept `OFFSET` only after a `LIMIT`.
                    None if offset > 0 && matches!(self, DatabaseBackend::Sqlite) => {
                        sql.push_str(" LIMIT -1");
                    }
                    None if offset > 0 && matches!(self, DatabaseBackend::Mysql) => {
                        sql.push_str(" LIMIT 18446744073709551615");
                    }
                    None => {}
                }
                if offset > 0 {
                    sql.push_str(&format!(" OFFSET {}", offset));
                }
                sql
            }
        }
    }

    fn relation_key_cast(&self, column: &str) -> String {
        match self {
            DatabaseBackend::Postgres => format!("CAST({column} AS TEXT)"),
            // Unbounded: `NVARCHAR(4000)` cuts a longer key, and two keys
            // that share their first 4,000 characters would read as one.
            DatabaseBackend::Mssql => format!("CAST({column} AS NVARCHAR(MAX))"),
            DatabaseBackend::Sqlite | DatabaseBackend::Mysql => format!("CAST({column} AS TEXT)"),
        }
    }

    fn current_epoch_expr(&self) -> &'static str {
        match self {
            DatabaseBackend::Postgres => "(EXTRACT(EPOCH FROM NOW())::bigint)",
            DatabaseBackend::Mssql => "DATEDIFF_BIG(second, '1970-01-01', SYSUTCDATETIME())",
            DatabaseBackend::Sqlite | DatabaseBackend::Mysql => "(unixepoch())",
        }
    }

    fn current_date_expr(&self) -> &'static str {
        match self {
            DatabaseBackend::Postgres => "CURRENT_DATE",
            DatabaseBackend::Mssql => "CAST(GETDATE() AS date)",
            DatabaseBackend::Sqlite | DatabaseBackend::Mysql => "date('now')",
        }
    }

    fn ci_like(&self, column: &str, placeholder: &str) -> String {
        match self {
            DatabaseBackend::Postgres => format!("{column} ILIKE {placeholder} ESCAPE '\\'"),
            DatabaseBackend::Sqlite | DatabaseBackend::Mysql | DatabaseBackend::Mssql => {
                format!("LOWER({column}) LIKE LOWER({placeholder}) ESCAPE '\\'")
            }
        }
    }

    fn days_ago_expr(&self, days: i64) -> String {
        match self {
            DatabaseBackend::Postgres => {
                format!("CURRENT_DATE - INTERVAL '{days} days'")
            }
            DatabaseBackend::Mssql => {
                format!("DATEADD(day, -{days}, CAST(GETDATE() AS date))")
            }
            DatabaseBackend::Sqlite | DatabaseBackend::Mysql => {
                format!("date('now', '-{days} days')")
            }
        }
    }

    fn days_ahead_expr(&self, days: i64) -> String {
        match self {
            DatabaseBackend::Postgres => {
                format!("CURRENT_DATE + INTERVAL '{days} days'")
            }
            DatabaseBackend::Mssql => {
                format!("DATEADD(day, {days}, CAST(GETDATE() AS date))")
            }
            DatabaseBackend::Sqlite | DatabaseBackend::Mysql => {
                format!("date('now', '+{days} days')")
            }
        }
    }

    fn spatial_geojson_expr(&self, placeholder: &str, srid: i32) -> String {
        match self {
            DatabaseBackend::Postgres => {
                format!("ST_SetSRID(ST_GeomFromGeoJSON({placeholder}::jsonb), {srid})")
            }
            DatabaseBackend::Sqlite | DatabaseBackend::Mysql | DatabaseBackend::Mssql => {
                format!("/* spatial unsupported on {} */ {placeholder}", self.name())
            }
        }
    }

    fn spatial_predicate(
        &self,
        predicate: SpatialPredicate,
        column: &str,
        geometry_expr: &str,
    ) -> String {
        match self {
            DatabaseBackend::Postgres => match predicate {
                SpatialPredicate::Equals => format!("ST_Equals({column}, {geometry_expr})"),
                SpatialPredicate::Disjoint => {
                    format!("NOT ST_Intersects({column}, {geometry_expr})")
                }
                SpatialPredicate::Intersects => {
                    format!("ST_Intersects({column}, {geometry_expr})")
                }
                SpatialPredicate::Touches => format!("ST_Touches({column}, {geometry_expr})"),
                SpatialPredicate::Crosses => format!("ST_Crosses({column}, {geometry_expr})"),
                SpatialPredicate::Within => format!("ST_Within({column}, {geometry_expr})"),
                SpatialPredicate::Contains => format!("ST_Contains({column}, {geometry_expr})"),
                SpatialPredicate::Overlaps => format!("ST_Overlaps({column}, {geometry_expr})"),
            },
            DatabaseBackend::Sqlite | DatabaseBackend::Mysql => {
                format!("FALSE /* spatial unsupported on {} */", self.name())
            }
            // T-SQL has no boolean literal. The macros refuse spatial fields
            // on SQL Server, so this is only a valid false predicate.
            DatabaseBackend::Mssql => "(1 = 0) /* spatial unsupported on mssql */".to_string(),
        }
    }
}

/// SQL Server text of a `float` or `real` expression that round-trips.
///
/// The default conversion keeps six significant digits (`1e+006`); style 3 is
/// the 17-digit form. A `real` is widened first so its text is the value Rust
/// reads when it widens the same `f32`.
pub(crate) fn mssql_float_text(expression: &str) -> String {
    format!("CONVERT(NVARCHAR(40), CAST({expression} AS FLOAT), 3)")
}

pub fn current_backend() -> DatabaseBackend {
    if cfg!(all(
        feature = "sqlite",
        not(any(feature = "postgres", feature = "mssql"))
    )) {
        DatabaseBackend::Sqlite
    } else if cfg!(all(
        feature = "postgres",
        not(any(feature = "sqlite", feature = "mssql"))
    )) {
        DatabaseBackend::Postgres
    } else if cfg!(all(
        feature = "mssql",
        not(any(feature = "sqlite", feature = "postgres"))
    )) {
        DatabaseBackend::Mssql
    } else {
        DatabaseBackend::Sqlite
    }
}

#[cfg(test)]
mod table_identifier_tests {
    use super::*;

    #[test]
    fn quoted_table_references_preserve_ddl_resolution() {
        for (declared, quoted) in [
            ("MixedNotes", "\"mixednotes\""),
            ("lower_notes", "\"lower_notes\""),
            ("Audit.MixedNotes", "\"audit\".\"mixednotes\""),
            ("\"Audit\".MixedNotes", "\"Audit\".\"mixednotes\""),
            ("\"MixedNotes\"", "\"MixedNotes\""),
            ("Mixed Notes", "\"Mixed Notes\""),
            ("Order", "\"Order\""),
        ] {
            assert_eq!(
                DatabaseBackend::Postgres.render_table_identifier_path(declared),
                quoted
            );
            for backend in [DatabaseBackend::Sqlite, DatabaseBackend::Mssql] {
                assert_eq!(
                    backend.render_table_identifier_path(declared),
                    backend.quote_identifier_path(declared)
                );
            }
        }
    }
}
