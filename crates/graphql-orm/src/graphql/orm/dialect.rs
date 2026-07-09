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
/// - leave string/blob literals and non-keyword identifiers untouched
///
/// Does **not** rewrite operators, function names, or argument order, so
/// genuinely different expressions remain distinct.
pub fn canonicalize_column_default_expression(default: &str) -> String {
    let mut value = default.trim().to_string();
    while is_fully_parenthesized(&value) {
        value = value[1..value.len() - 1].trim().to_string();
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

pub trait SqlDialect {
    fn backend(&self) -> DatabaseBackend;
    fn quote_identifier(&self, identifier: &str) -> String;
    fn quote_identifier_path(&self, identifier: &str) -> String;
    fn placeholder(&self, index: usize) -> String;
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
                if identifier.starts_with('[') && identifier.ends_with(']') {
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
                if identifier.starts_with('"') && identifier.ends_with('"') {
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

        identifier
            .split('.')
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
