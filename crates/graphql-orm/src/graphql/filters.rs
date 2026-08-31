#[derive(async_graphql::InputObject, Clone, Debug, Default)]
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[cfg_attr(
    feature = "field-case-screaming-snake",
    graphql(rename_fields = "SCREAMING_SNAKE_CASE")
)]
#[cfg_attr(feature = "field-case-lower", graphql(rename_fields = "lowercase"))]
#[cfg_attr(feature = "field-case-upper", graphql(rename_fields = "UPPERCASE"))]
pub struct SimilarityInput {
    pub value: String,
}

#[derive(
    async_graphql::Enum, serde::Serialize, serde::Deserialize, Copy, Clone, Debug, Eq, PartialEq,
)]
/// Full-text search query interpretation used by generated search resolvers.
pub enum SearchMode {
    /// Tokenize the query as plain words and require backend-default term matching.
    Plain,
    /// Treat the query as a phrase where the backend supports phrase search.
    Phrase,
    /// Use web-search style syntax where the backend supports it.
    Web,
    /// Match sanitized query tokens as prefixes.
    Prefix,
}

impl Default for SearchMode {
    fn default() -> Self {
        Self::Plain
    }
}

#[derive(async_graphql::InputObject, Clone, Debug)]
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[cfg_attr(
    feature = "field-case-screaming-snake",
    graphql(rename_fields = "SCREAMING_SNAKE_CASE")
)]
#[cfg_attr(feature = "field-case-lower", graphql(rename_fields = "lowercase"))]
#[cfg_attr(feature = "field-case-upper", graphql(rename_fields = "UPPERCASE"))]
/// Input accepted by generated per-entity full-text search resolvers.
pub struct SearchInput {
    /// User-provided search text.
    pub query: String,
    /// Optional query mode. Defaults to [`SearchMode::Plain`].
    pub mode: Option<SearchMode>,
    /// Optional minimum relevance score applied before results are returned.
    #[cfg_attr(feature = "field-case-lower", graphql(name = "minscore"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "MINSCORE"))]
    pub min_score: Option<f64>,
}

#[derive(async_graphql::InputObject, Clone, Debug, Default)]
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[cfg_attr(
    feature = "field-case-screaming-snake",
    graphql(rename_fields = "SCREAMING_SNAKE_CASE")
)]
#[cfg_attr(feature = "field-case-lower", graphql(rename_fields = "lowercase"))]
#[cfg_attr(feature = "field-case-upper", graphql(rename_fields = "UPPERCASE"))]
pub struct UuidFilter {
    pub eq: Option<uuid::Uuid>,
    pub ne: Option<uuid::Uuid>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "inlist"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "INLIST"))]
    pub in_list: Option<Vec<uuid::Uuid>>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "notin"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "NOTIN"))]
    pub not_in: Option<Vec<uuid::Uuid>>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "isnull"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "ISNULL"))]
    pub is_null: Option<bool>,
}

/// Exact-match predicates for binary columns. Values remain raw bytes.
#[derive(async_graphql::InputObject, Clone, Debug, Default)]
pub struct BytesFilter {
    pub eq: Option<Vec<u8>>,
    pub ne: Option<Vec<u8>>,
    pub in_list: Option<Vec<Vec<u8>>>,
    pub not_in: Option<Vec<Vec<u8>>>,
    pub is_null: Option<bool>,
}

#[derive(async_graphql::InputObject, Clone, Debug, Default)]
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[cfg_attr(
    feature = "field-case-screaming-snake",
    graphql(rename_fields = "SCREAMING_SNAKE_CASE")
)]
#[cfg_attr(feature = "field-case-lower", graphql(rename_fields = "lowercase"))]
#[cfg_attr(feature = "field-case-upper", graphql(rename_fields = "UPPERCASE"))]
pub struct StringFilter {
    pub eq: Option<String>,
    pub ne: Option<String>,
    pub contains: Option<String>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "startswith"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "STARTSWITH"))]
    pub starts_with: Option<String>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "endswith"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "ENDSWITH"))]
    pub ends_with: Option<String>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "inlist"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "INLIST"))]
    pub in_list: Option<Vec<String>>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "notin"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "NOTIN"))]
    pub not_in: Option<Vec<String>>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "isnull"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "ISNULL"))]
    pub is_null: Option<bool>,
    pub similar: Option<SimilarityInput>,
}

#[derive(async_graphql::InputObject, Clone, Debug, Default)]
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[cfg_attr(
    feature = "field-case-screaming-snake",
    graphql(rename_fields = "SCREAMING_SNAKE_CASE")
)]
#[cfg_attr(feature = "field-case-lower", graphql(rename_fields = "lowercase"))]
#[cfg_attr(feature = "field-case-upper", graphql(rename_fields = "UPPERCASE"))]
/// Spatial predicate filter for GeoJSON geometry fields.
pub struct SpatialFilter {
    /// Topological equality predicate.
    pub equals: Option<async_graphql::Json<serde_json::Value>>,
    /// Topological disjoint predicate.
    pub disjoint: Option<async_graphql::Json<serde_json::Value>>,
    /// Topological intersection predicate.
    pub intersects: Option<async_graphql::Json<serde_json::Value>>,
    /// Topological touches predicate.
    pub touches: Option<async_graphql::Json<serde_json::Value>>,
    /// Topological crosses predicate.
    pub crosses: Option<async_graphql::Json<serde_json::Value>>,
    /// Topological within predicate.
    pub within: Option<async_graphql::Json<serde_json::Value>>,
    /// Topological contains predicate.
    pub contains: Option<async_graphql::Json<serde_json::Value>>,
    /// Topological overlaps predicate.
    pub overlaps: Option<async_graphql::Json<serde_json::Value>>,
    /// Null check. When set, no geometry value is bound for this predicate.
    #[cfg_attr(feature = "field-case-lower", graphql(name = "isnull"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "ISNULL"))]
    pub is_null: Option<bool>,
}

#[derive(async_graphql::InputObject, Clone, Debug, Default)]
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[cfg_attr(
    feature = "field-case-screaming-snake",
    graphql(rename_fields = "SCREAMING_SNAKE_CASE")
)]
#[cfg_attr(feature = "field-case-lower", graphql(rename_fields = "lowercase"))]
#[cfg_attr(feature = "field-case-upper", graphql(rename_fields = "UPPERCASE"))]
pub struct IntFilter {
    pub eq: Option<i32>,
    pub ne: Option<i32>,
    pub lt: Option<i32>,
    pub lte: Option<i32>,
    pub gt: Option<i32>,
    pub gte: Option<i32>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "inlist"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "INLIST"))]
    pub in_list: Option<Vec<i32>>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "notin"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "NOTIN"))]
    pub not_in: Option<Vec<i32>>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "isnull"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "ISNULL"))]
    pub is_null: Option<bool>,
}

/// Exact comparison predicates for a fixed-precision decimal column.
#[derive(async_graphql::InputObject, Clone, Debug, Default)]
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[cfg_attr(
    feature = "field-case-screaming-snake",
    graphql(rename_fields = "SCREAMING_SNAKE_CASE")
)]
#[cfg_attr(feature = "field-case-lower", graphql(rename_fields = "lowercase"))]
#[cfg_attr(feature = "field-case-upper", graphql(rename_fields = "UPPERCASE"))]
pub struct DecimalFilter {
    /// Equals.
    pub eq: Option<rust_decimal::Decimal>,
    /// Does not equal.
    pub ne: Option<rust_decimal::Decimal>,
    /// Less than.
    pub lt: Option<rust_decimal::Decimal>,
    /// Less than or equal.
    pub lte: Option<rust_decimal::Decimal>,
    /// Greater than.
    pub gt: Option<rust_decimal::Decimal>,
    /// Greater than or equal.
    pub gte: Option<rust_decimal::Decimal>,
    /// Null predicate.
    #[cfg_attr(feature = "field-case-lower", graphql(name = "isnull"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "ISNULL"))]
    pub is_null: Option<bool>,
}

#[derive(async_graphql::InputObject, Clone, Debug, Default)]
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[cfg_attr(
    feature = "field-case-screaming-snake",
    graphql(rename_fields = "SCREAMING_SNAKE_CASE")
)]
#[cfg_attr(feature = "field-case-lower", graphql(rename_fields = "lowercase"))]
#[cfg_attr(feature = "field-case-upper", graphql(rename_fields = "UPPERCASE"))]
pub struct BoolFilter {
    pub eq: Option<bool>,
    pub ne: Option<bool>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "isnull"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "ISNULL"))]
    pub is_null: Option<bool>,
}

#[derive(async_graphql::InputObject, Clone, Debug, Default)]
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[cfg_attr(
    feature = "field-case-screaming-snake",
    graphql(rename_fields = "SCREAMING_SNAKE_CASE")
)]
#[cfg_attr(feature = "field-case-lower", graphql(rename_fields = "lowercase"))]
#[cfg_attr(feature = "field-case-upper", graphql(rename_fields = "UPPERCASE"))]
pub struct DateRangeInput {
    /// Inclusive ISO-8601 lower bound.
    pub start: String,
    /// Inclusive ISO-8601 upper bound.
    pub end: String,
}

/// Maximum positive calendar span accepted by `recentDays` and `withinDays`.
///
/// The 100-year ceiling is backend-neutral and remains within the supported
/// date range of SQLite, PostgreSQL, and SQL Server for contemporary clocks.
pub const MAX_CALENDAR_DAY_SPAN: i32 = 36_600;

/// Maximum absolute calendar offset accepted by relative-date predicates.
pub const MAX_RELATIVE_DAY_OFFSET: i32 = 36_600;

#[derive(async_graphql::InputObject, Clone, Debug, Default)]
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[cfg_attr(
    feature = "field-case-screaming-snake",
    graphql(rename_fields = "SCREAMING_SNAKE_CASE")
)]
#[cfg_attr(feature = "field-case-lower", graphql(rename_fields = "lowercase"))]
#[cfg_attr(feature = "field-case-upper", graphql(rename_fields = "UPPERCASE"))]
pub struct RelativeDateInput {
    /// Signed calendar-day offset from the start of today. The accepted range
    /// is -36,600 through 36,600 days.
    pub days: i32,
}

impl RelativeDateInput {
    pub fn to_sql_expr(&self, backend: crate::graphql::orm::DatabaseBackend) -> String {
        let days = i64::from(self.days);
        if days < 0 {
            crate::graphql::orm::SqlDialect::days_ago_expr(&backend, days.abs())
        } else {
            crate::graphql::orm::SqlDialect::days_ahead_expr(&backend, days)
        }
    }
}

#[derive(async_graphql::InputObject, Clone, Debug, Default)]
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[cfg_attr(
    feature = "field-case-screaming-snake",
    graphql(rename_fields = "SCREAMING_SNAKE_CASE")
)]
#[cfg_attr(feature = "field-case-lower", graphql(rename_fields = "lowercase"))]
#[cfg_attr(feature = "field-case-upper", graphql(rename_fields = "UPPERCASE"))]
pub struct DateFilter {
    pub eq: Option<String>,
    pub ne: Option<String>,
    pub lt: Option<String>,
    pub lte: Option<String>,
    pub gt: Option<String>,
    pub gte: Option<String>,
    pub between: Option<DateRangeInput>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "isnull"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "ISNULL"))]
    pub is_null: Option<bool>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "inpast"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "INPAST"))]
    pub in_past: Option<bool>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "infuture"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "INFUTURE"))]
    pub in_future: Option<bool>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "istoday"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "ISTODAY"))]
    pub is_today: Option<bool>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "recentdays"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "RECENTDAYS"))]
    /// Positive number of calendar dates ending with today (maximum 36,600).
    #[graphql(validator(minimum = 1, maximum = 36600))]
    pub recent_days: Option<i32>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "withindays"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "WITHINDAYS"))]
    /// Positive number of calendar dates beginning with today (maximum 36,600).
    #[graphql(validator(minimum = 1, maximum = 36600))]
    pub within_days: Option<i32>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "gterelative"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "GTERELATIVE"))]
    pub gte_relative: Option<RelativeDateInput>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "lterelative"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "LTERELATIVE"))]
    pub lte_relative: Option<RelativeDateInput>,
}

impl DateFilter {
    /// Validate bounds shared by GraphQL-decoded and programmatically built filters.
    pub fn validate(&self) -> crate::Result<()> {
        if let Some(days) = self.recent_days {
            validate_positive_calendar_span("recentDays", days)?;
        }
        if let Some(days) = self.within_days {
            validate_positive_calendar_span("withinDays", days)?;
        }
        if let Some(relative) = &self.gte_relative {
            validate_relative_offset("gteRelative", relative.days)?;
        }
        if let Some(relative) = &self.lte_relative {
            validate_relative_offset("lteRelative", relative.days)?;
        }
        if let Some(range) = &self.between {
            let start = parse_comparable_date_value(&range.start).ok_or_else(|| {
                invalid_date_filter("between.start must be an ISO-8601 date or timestamp")
            })?;
            let end = parse_comparable_date_value(&range.end).ok_or_else(|| {
                invalid_date_filter("between.end must be an ISO-8601 date or timestamp")
            })?;
            if start > end {
                return Err(invalid_date_filter(
                    "between.start must not be after between.end",
                ));
            }
        }
        Ok(())
    }

    /// Render this filter for macro-generated SQL.
    ///
    /// This method is public only because procedural-macro expansion occurs in
    /// the consuming crate. The column expression is fixed generated metadata;
    /// GraphQL clients cannot supply SQL or identifiers.
    #[doc(hidden)]
    pub fn render_sql(
        &self,
        backend: crate::graphql::orm::DatabaseBackend,
        column: &str,
        start_index: usize,
    ) -> crate::Result<(Vec<String>, Vec<crate::graphql::orm::SqlValue>)> {
        self.render_sql_with_calendar(backend, column, start_index, true)
    }

    /// Render only clock-independent predicates for a residual in-memory path.
    #[doc(hidden)]
    pub fn render_sql_prefilter(
        &self,
        backend: crate::graphql::orm::DatabaseBackend,
        column: &str,
        start_index: usize,
    ) -> crate::Result<(Vec<String>, Vec<crate::graphql::orm::SqlValue>)> {
        self.render_sql_with_calendar(backend, column, start_index, false)
    }

    fn render_sql_with_calendar(
        &self,
        backend: crate::graphql::orm::DatabaseBackend,
        column: &str,
        start_index: usize,
        include_calendar: bool,
    ) -> crate::Result<(Vec<String>, Vec<crate::graphql::orm::SqlValue>)> {
        use crate::graphql::orm::{SqlDialect, SqlValue};

        self.validate()?;
        let mut conditions = Vec::new();
        let mut values = Vec::new();
        for (value, operator) in [
            (&self.eq, "="),
            (&self.ne, "!="),
            (&self.lt, "<"),
            (&self.lte, "<="),
            (&self.gt, ">"),
            (&self.gte, ">="),
        ] {
            if let Some(value) = value {
                let placeholder = backend.placeholder(start_index + values.len());
                conditions.push(format!("{column} {operator} {placeholder}"));
                values.push(SqlValue::String(value.clone()));
            }
        }
        if let Some(range) = &self.between {
            let start_placeholder = backend.placeholder(start_index + values.len());
            let end_placeholder = backend.placeholder(start_index + values.len() + 1);
            conditions.push(format!(
                "{column} BETWEEN {start_placeholder} AND {end_placeholder}"
            ));
            values.push(SqlValue::String(range.start.clone()));
            values.push(SqlValue::String(range.end.clone()));
        }
        if let Some(is_null) = self.is_null {
            conditions.push(format!(
                "{column} IS {}NULL",
                if is_null { "" } else { "NOT " }
            ));
        }

        if include_calendar {
            let today = backend.current_date_expr();
            let tomorrow = backend.days_ahead_expr(1);
            if self.in_past == Some(true) {
                conditions.push(format!("{column} < {today}"));
            }
            if self.in_future == Some(true) {
                conditions.push(format!("{column} >= {tomorrow}"));
            }
            if self.is_today == Some(true) {
                conditions.push(format!("{column} >= {today} AND {column} < {tomorrow}"));
            }
            if let Some(days) = self.recent_days {
                let lower = backend.days_ago_expr(i64::from(days - 1));
                conditions.push(format!("{column} >= {lower} AND {column} < {tomorrow}"));
            }
            if let Some(days) = self.within_days {
                let upper = backend.days_ahead_expr(i64::from(days));
                conditions.push(format!("{column} >= {today} AND {column} < {upper}"));
            }
            if let Some(relative) = &self.gte_relative {
                conditions.push(format!("{column} >= {}", relative.to_sql_expr(backend)));
            }
            if let Some(relative) = &self.lte_relative {
                let exclusive_offset = i64::from(relative.days) + 1;
                conditions.push(format!(
                    "{column} < {}",
                    relative_day_expr(backend, exclusive_offset)
                ));
            }
        }

        Ok((conditions, values))
    }
}

fn validate_positive_calendar_span(name: &str, days: i32) -> crate::Result<()> {
    if !(1..=MAX_CALENDAR_DAY_SPAN).contains(&days) {
        return Err(invalid_date_filter(format!(
            "{name} must be between 1 and {MAX_CALENDAR_DAY_SPAN} days"
        )));
    }
    Ok(())
}

fn validate_relative_offset(name: &str, days: i32) -> crate::Result<()> {
    if !(-MAX_RELATIVE_DAY_OFFSET..=MAX_RELATIVE_DAY_OFFSET).contains(&days) {
        return Err(invalid_date_filter(format!(
            "{name}.days must be between -{MAX_RELATIVE_DAY_OFFSET} and {MAX_RELATIVE_DAY_OFFSET}"
        )));
    }
    Ok(())
}

fn invalid_date_filter(message: impl Into<String>) -> sqlx::Error {
    crate::graphql::errors::sqlx_error_from_public(
        crate::graphql::errors::OrmPublicError::new(
            crate::graphql::errors::OrmErrorCode::InvalidInput,
        )
        .with_internal(message.into()),
    )
}

fn relative_day_expr(backend: crate::graphql::orm::DatabaseBackend, days: i64) -> String {
    use crate::graphql::orm::SqlDialect;

    if days < 0 {
        backend.days_ago_expr(days.unsigned_abs() as i64)
    } else {
        backend.days_ahead_expr(days)
    }
}

fn parse_comparable_date_value(value: &str) -> Option<chrono::NaiveDateTime> {
    if let Ok(value) = chrono::DateTime::parse_from_rfc3339(value) {
        return Some(value.naive_utc());
    }
    for format in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"] {
        if let Ok(value) = chrono::NaiveDateTime::parse_from_str(value, format) {
            return Some(value);
        }
    }
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .and_then(|value| value.and_hms_opt(0, 0, 0))
}
