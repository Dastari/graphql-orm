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

fn normalize_sql_placeholders(backend: DatabaseBackend, sql: &str, start_index: usize) -> String {
    if !matches!(backend, DatabaseBackend::Postgres | DatabaseBackend::Mssql) {
        return sql.to_string();
    }

    let chars: Vec<char> = sql.chars().collect();
    let mut out = String::with_capacity(sql.len() + 16);
    let mut i = 0usize;
    let mut next = start_index;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut in_bracket_quote = false;

    while i < chars.len() {
        let ch = chars[i];

        if in_single_quote {
            out.push(ch);
            if ch == '\'' {
                if i + 1 < chars.len() && chars[i + 1] == '\'' {
                    out.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                in_single_quote = false;
            }
            i += 1;
            continue;
        }

        if in_double_quote {
            out.push(ch);
            if ch == '"' {
                if i + 1 < chars.len() && chars[i + 1] == '"' {
                    out.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                in_double_quote = false;
            }
            i += 1;
            continue;
        }

        if in_bracket_quote {
            out.push(ch);
            if ch == ']' {
                if i + 1 < chars.len() && chars[i + 1] == ']' {
                    out.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                in_bracket_quote = false;
            }
            i += 1;
            continue;
        }

        if ch == '\'' {
            in_single_quote = true;
            out.push(ch);
            i += 1;
            continue;
        }
        if ch == '"' {
            in_double_quote = true;
            out.push(ch);
            i += 1;
            continue;
        }
        if backend == DatabaseBackend::Mssql && ch == '[' {
            in_bracket_quote = true;
            out.push(ch);
            i += 1;
            continue;
        }

        if ch == '?'
            || ch == '$'
            || (ch == '@' && i + 1 < chars.len() && chars[i + 1].eq_ignore_ascii_case(&'p'))
        {
            out.push_str(&backend.placeholder(next));
            next += 1;
            i += if ch == '@' { 2 } else { 1 };
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
        } else {
            out.push(ch);
            i += 1;
        }
    }

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

impl DatabaseBackend {
    fn reserved_identifiers(self) -> &'static [&'static str] {
        match self {
            Self::Sqlite => SQLITE_RESERVED_IDENTIFIERS,
            Self::Postgres => POSTGRES_RESERVED_IDENTIFIERS,
            Self::Mssql => MSSQL_RESERVED_IDENTIFIERS,
            Self::Mysql => &[],
        }
    }

    fn identifier_needs_quoting(self, identifier: &str) -> bool {
        if self == Self::Mysql || self.quote_identifier(identifier) == identifier {
            // MySQL identifiers are rendered verbatim, and an identifier that is
            // already delimited is passed through unchanged.
            return false;
        }
        let mut characters = identifier.chars();
        let Some(first) = characters.next() else {
            return true;
        };
        // Non-ASCII characters are identifier characters on every backend and stay
        // bare, so their case folding is unchanged. MSSQL `#`/`@` prefixes keep
        // their temporary-table and variable meaning.
        let plain_start = |character: char| {
            character.is_ascii_alphabetic()
                || character == '_'
                || !character.is_ascii()
                || (self == Self::Mssql && matches!(character, '#' | '@'))
        };
        let plain_rest = |character: char| {
            plain_start(character) || character.is_ascii_digit() || character == '$'
        };
        if !plain_start(first) || !characters.all(plain_rest) {
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
    /// that was already accepted. Reserved words such as `order`, `join` or
    /// `select`, and names with other characters, are delimited with
    /// [`SqlDialect::quote_identifier`]; already delimited input is returned
    /// unchanged. MySQL identifiers are returned unchanged.
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
        if !identifier_path_parts(identifier)
            .any(|part| !part.is_empty() && self.identifier_needs_quoting(part))
        {
            return std::borrow::Cow::Borrowed(identifier);
        }
        std::borrow::Cow::Owned(
            identifier_path_parts(identifier)
                .filter(|part| !part.is_empty())
                .map(|part| self.render_identifier(part))
                .collect::<Vec<_>>()
                .join("."),
        )
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
        // Already delimited input is not delimited twice.
        assert_eq!(sqlite.render_identifier("\"join\""), "\"join\"");
        assert_eq!(mssql.render_identifier("[key]"), "[key]");
        assert_eq!(DatabaseBackend::Mysql.render_identifier("order"), "order");
    }

    #[test]
    fn macro_reserved_identifier_lists_match_the_runtime() {
        // The derive crate cannot depend on this crate, so it carries copies of
        // the lists. Compare them when the workspace sources are present.
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
    fn quote_identifier(&self, identifier: &str) -> String;
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
            DatabaseBackend::Mssql => {
                if (identifier.starts_with('[') && identifier.ends_with(']'))
                    || (identifier.starts_with('"') && identifier.ends_with('"'))
                {
                    identifier.to_string()
                } else {
                    format!("[{}]", identifier.replace(']', "]]"))
                }
            }
            DatabaseBackend::Postgres => {
                if identifier.starts_with('"') && identifier.ends_with('"') {
                    identifier.to_string()
                } else {
                    format!("\"{}\"", identifier.replace('"', "\"\""))
                }
            }
            DatabaseBackend::Sqlite => {
                if (identifier.starts_with('"') && identifier.ends_with('"'))
                    || (identifier.starts_with('[') && identifier.ends_with(']'))
                    || (identifier.starts_with('`') && identifier.ends_with('`'))
                {
                    identifier.to_string()
                } else {
                    format!("\"{}\"", identifier.replace('"', "\"\""))
                }
            }
            DatabaseBackend::Mysql => identifier.to_string(),
        }
    }

    fn quote_identifier_path(&self, identifier: &str) -> String {
        if *self != DatabaseBackend::Mssql
            && *self != DatabaseBackend::Postgres
            && *self != DatabaseBackend::Sqlite
        {
            return identifier.to_string();
        }

        identifier_path_parts(identifier)
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
                Some(limit) => {
                    format!(
                        " OFFSET {} ROWS FETCH NEXT {} ROWS ONLY",
                        offset.max(0),
                        limit.max(0)
                    )
                }
                None if offset > 0 => format!(" OFFSET {} ROWS", offset),
                None => String::new(),
            },
            _ => {
                let mut sql = String::new();
                if let Some(limit) = limit {
                    sql.push_str(&format!(" LIMIT {}", limit.max(0)));
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
            DatabaseBackend::Mssql => format!("CAST({column} AS NVARCHAR(4000))"),
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
            DatabaseBackend::Sqlite | DatabaseBackend::Mysql | DatabaseBackend::Mssql => {
                format!("FALSE /* spatial unsupported on {} */", self.name())
            }
        }
    }
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
