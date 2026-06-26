#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum DatabaseBackend {
    Sqlite,
    Postgres,
    Mysql,
    Mssql,
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
            DatabaseBackend::Sqlite | DatabaseBackend::Mysql => identifier.to_string(),
        }
    }

    fn quote_identifier_path(&self, identifier: &str) -> String {
        if *self != DatabaseBackend::Mssql && *self != DatabaseBackend::Postgres {
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
        if !matches!(self, DatabaseBackend::Postgres | DatabaseBackend::Mssql) {
            return sql.to_string();
        }

        let chars: Vec<char> = sql.chars().collect();
        let mut out = String::with_capacity(sql.len() + 16);
        let mut i = 0usize;
        let mut next = start_index;
        while i < chars.len() {
            if chars[i] == '?'
                || chars[i] == '$'
                || (chars[i] == '@'
                    && i + 1 < chars.len()
                    && chars[i + 1].eq_ignore_ascii_case(&'p'))
            {
                out.push_str(&self.placeholder(next));
                next += 1;
                i += if chars[i] == '@' { 2 } else { 1 };
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
            } else {
                out.push(chars[i]);
                i += 1;
            }
        }
        out
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
                        limit
                    )
                }
                None if offset > 0 => format!(" OFFSET {} ROWS", offset),
                None => String::new(),
            },
            _ => {
                let mut sql = String::new();
                if let Some(limit) = limit {
                    sql.push_str(&format!(" LIMIT {}", limit));
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
            DatabaseBackend::Postgres => format!("{column} ILIKE {placeholder}"),
            DatabaseBackend::Sqlite | DatabaseBackend::Mysql | DatabaseBackend::Mssql => {
                format!("LOWER({column}) LIKE LOWER({placeholder})")
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
