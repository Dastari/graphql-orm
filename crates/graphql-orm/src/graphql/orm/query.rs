#[cfg(feature = "mssql")]
use super::MssqlBackend;
#[cfg(feature = "postgres")]
use super::PostgresBackend;
#[cfg(feature = "sqlite")]
use super::SqliteBackend;
use super::core::{
    ColumnDef, DbAuthContext, EntityMetadata, IndexDef, RelationMetadata, SchemaPolicy,
    SearchIndexDef, SqlValue,
};
use super::dialect::{DatabaseBackend, SqlDialect, current_backend};
use super::{DefaultBackend, OrmBackend, SqlxBackend};
use crate::graphql::pagination::{Connection, Edge, PageInfo, encode_cursor};
use std::any::Any;
use std::marker::PhantomData;
use std::sync::Arc;

pub trait DatabaseEntity {
    const TABLE_NAME: &'static str;
    const PLURAL_NAME: &'static str;
    /// Compatibility accessor for the first primary-key column.
    const PRIMARY_KEY: &'static str;
    /// All primary-key columns in declaration order.
    const PRIMARY_KEYS: &'static [&'static str] = &[Self::PRIMARY_KEY];
    const SCHEMA_POLICY: Option<SchemaPolicy> = None;
    const DEFAULT_SORT: &'static str;

    fn column_names() -> &'static [&'static str];
}

pub trait DatabaseSchema {
    fn columns() -> &'static [ColumnDef];
    fn indexes() -> &'static [IndexDef];
    fn composite_unique_indexes() -> &'static [&'static [&'static str]];
}

pub trait EntityRelations {
    fn relation_metadata() -> &'static [RelationMetadata] {
        &[]
    }
}

pub trait DatabaseSearchSchema {
    fn search_index() -> Option<&'static SearchIndexDef> {
        None
    }
}

pub trait Entity: DatabaseEntity + DatabaseSchema + EntityRelations + DatabaseSearchSchema {
    fn entity_name() -> &'static str;
    fn metadata() -> &'static EntityMetadata;
}

/// Optional generated PostgreSQL row-level security metadata for an entity.
///
/// Entities without `#[graphql_rls]` use the default `None` implementation and
/// keep existing schema-management behavior.
pub trait DatabaseRls {
    /// Return generated RLS metadata for this entity, when configured.
    fn rls_metadata() -> Option<&'static super::core::RlsEntityMetadata> {
        None
    }
}

pub trait FromSqlRow<B: OrmBackend = DefaultBackend>: Sized {
    fn from_row(row: &B::Row) -> Result<Self, sqlx::Error>;
}

pub trait DatabaseFilter {
    fn to_sql_conditions(&self) -> (Vec<String>, Vec<SqlValue>);
    fn is_empty(&self) -> bool;

    /// Return true when at least one predicate must be evaluated against decoded
    /// Rust entities instead of being rendered entirely into SQL.
    fn requires_in_memory_filtering(&self, _backend: DatabaseBackend) -> bool {
        false
    }

    /// Return SQL predicates that are safe to apply before a residual in-memory
    /// matcher runs.
    ///
    /// Generated filters override this for backends such as SQLite spatial
    /// fallback, where ordinary field predicates can still shrink the candidate
    /// set before exact topology checks run in Rust.
    fn to_sql_prefilter_conditions(
        &self,
        backend: DatabaseBackend,
    ) -> (Vec<String>, Vec<SqlValue>) {
        if self.requires_in_memory_filtering(backend) {
            (Vec::new(), Vec::new())
        } else {
            self.to_sql_conditions()
        }
    }

    fn matches_entity(&self, _entity: &(dyn Any + Send + Sync)) -> Result<bool, sqlx::Error> {
        Ok(true)
    }

    fn to_filter_expression(&self) -> Option<FilterExpression> {
        let (conditions, values) = self.to_sql_conditions();
        filter_expression_from_raw_parts(&conditions, &values)
    }
}

pub trait DatabaseOrderBy {
    fn to_sql_order(&self) -> Option<String>;

    fn to_sort_expression(&self) -> Option<SortExpression> {
        self.to_sql_order().map(|clause| SortExpression { clause })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum FilterExpression {
    Raw {
        clause: String,
        values: Vec<SqlValue>,
    },
    And(Vec<FilterExpression>),
    Or(Vec<FilterExpression>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SortExpression {
    pub clause: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderedQuery {
    pub sql: String,
    pub values: Vec<SqlValue>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SelectQuery {
    pub table: &'static str,
    pub columns: Vec<String>,
    pub filter: Option<FilterExpression>,
    pub sorts: Vec<SortExpression>,
    pub pagination: Option<PaginationRequest>,
    pub count_only: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AggregateFunction {
    Count,
    Max,
    Min,
}

impl AggregateFunction {
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Count => "COUNT",
            Self::Max => "MAX",
            Self::Min => "MIN",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AggregateQuery {
    pub table: &'static str,
    pub function: AggregateFunction,
    pub column: Option<String>,
    pub filter: Option<FilterExpression>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeleteQuery {
    pub table: &'static str,
    pub filter: Option<FilterExpression>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PaginationConfig {
    /// Limit applied to connection-style queries when the request omits a limit.
    pub default_limit: Option<i64>,
    /// Maximum accepted explicit or default limit.
    pub max_limit: Option<i64>,
}

impl Default for PaginationConfig {
    fn default() -> Self {
        Self {
            default_limit: Some(Self::DEFAULT_LIMIT),
            max_limit: Some(Self::DEFAULT_MAX_LIMIT),
        }
    }
}

impl PaginationConfig {
    /// Default connection limit used when a request omits `PageInput.limit`.
    pub const DEFAULT_LIMIT: i64 = 1000;
    /// Default cap applied to explicit and default limits.
    pub const DEFAULT_MAX_LIMIT: i64 = 1000;

    /// Create a config that does not add a default limit and does not cap limits.
    pub const fn unbounded() -> Self {
        Self {
            default_limit: None,
            max_limit: None,
        }
    }

    /// Create a config with the default limit disabled but explicit limits capped.
    pub const fn explicit_only(max_limit: i64) -> Self {
        Self {
            default_limit: None,
            max_limit: Some(max_limit),
        }
    }

    /// Return a copy with a different default limit.
    pub fn with_default_limit(mut self, default_limit: Option<i64>) -> Self {
        self.default_limit = default_limit;
        self
    }

    /// Return a copy with a different maximum limit.
    pub fn with_max_limit(mut self, max_limit: Option<i64>) -> Self {
        self.max_limit = max_limit;
        self
    }

    /// Clamp one explicit limit without applying a default.
    pub fn clamp_explicit_limit(&self, limit: Option<i64>) -> Option<i64> {
        limit.map(|limit| self.clamp_limit_value(limit))
    }

    /// Resolve an optional page input into an offset/limit pair.
    ///
    /// When `apply_default_limit` is true, `default_limit` is used if the page
    /// input or its `limit` field is absent. Repository-style `fetch_all` paths
    /// pass false so callers can intentionally fetch all rows.
    pub fn resolve_page(
        &self,
        page: Option<&PageInput>,
        apply_default_limit: bool,
    ) -> PaginationRequest {
        let offset = page.map(PageInput::offset).unwrap_or(0);
        let requested_limit = page.and_then(|page| page.limit);
        let limit = if requested_limit.is_some() {
            self.clamp_explicit_limit(requested_limit)
        } else if apply_default_limit {
            self.clamp_explicit_limit(self.default_limit)
        } else {
            None
        };
        PaginationRequest { limit, offset }
    }

    fn clamp_limit_value(&self, limit: i64) -> i64 {
        let normalized = limit.max(0);
        match self.max_limit {
            Some(max_limit) => normalized.min(max_limit.max(0)),
            None => normalized,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PaginationRequest {
    pub limit: Option<i64>,
    pub offset: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchemaLimits {
    /// Maximum GraphQL query depth applied by generated `schema_builder` helpers.
    pub max_depth: Option<usize>,
    /// Maximum GraphQL query complexity applied by generated `schema_builder` helpers.
    pub max_complexity: Option<usize>,
}

impl Default for SchemaLimits {
    fn default() -> Self {
        Self {
            max_depth: Some(Self::DEFAULT_MAX_DEPTH),
            max_complexity: Some(Self::DEFAULT_MAX_COMPLEXITY),
        }
    }
}

impl SchemaLimits {
    /// Default GraphQL query depth cap for generated schemas.
    pub const DEFAULT_MAX_DEPTH: usize = 16;
    /// Default GraphQL query complexity cap for generated schemas.
    pub const DEFAULT_MAX_COMPLEXITY: usize = 20_000;

    /// Create limits with both depth and complexity checks disabled.
    pub const fn unbounded() -> Self {
        Self {
            max_depth: None,
            max_complexity: None,
        }
    }

    /// Return a copy with a different depth cap.
    pub const fn with_max_depth(mut self, max_depth: Option<usize>) -> Self {
        self.max_depth = max_depth;
        self
    }

    /// Return a copy with a different complexity cap.
    pub const fn with_max_complexity(mut self, max_complexity: Option<usize>) -> Self {
        self.max_complexity = max_complexity;
        self
    }

    /// Apply these limits to an async-graphql schema builder.
    pub fn apply<Query, Mutation, Subscription>(
        self,
        mut builder: async_graphql::SchemaBuilder<Query, Mutation, Subscription>,
    ) -> async_graphql::SchemaBuilder<Query, Mutation, Subscription> {
        if let Some(max_depth) = self.max_depth {
            builder = builder.limit_depth(max_depth);
        }
        if let Some(max_complexity) = self.max_complexity {
            builder = builder.limit_complexity(max_complexity);
        }
        builder
    }
}

impl From<&PageInput> for PaginationRequest {
    fn from(value: &PageInput) -> Self {
        PaginationConfig::default().resolve_page(Some(value), false)
    }
}

#[derive(async_graphql::Enum, Copy, Clone, Debug, Eq, PartialEq)]
pub enum OrderDirection {
    Asc,
    Desc,
}

impl OrderDirection {
    pub fn to_sql(self) -> &'static str {
        match self {
            Self::Asc => "ASC",
            Self::Desc => "DESC",
        }
    }
}

impl DatabaseFilter for () {
    fn to_sql_conditions(&self) -> (Vec<String>, Vec<SqlValue>) {
        (Vec::new(), Vec::new())
    }

    fn is_empty(&self) -> bool {
        true
    }
}

impl DatabaseOrderBy for () {
    fn to_sql_order(&self) -> Option<String> {
        None
    }
}

#[derive(
    async_graphql::Enum, serde::Serialize, serde::Deserialize, Copy, Clone, Debug, Eq, PartialEq,
)]
pub enum ChangeAction {
    Created,
    Updated,
    Deleted,
}

#[derive(
    async_graphql::Enum, serde::Serialize, serde::Deserialize, Copy, Clone, Debug, Eq, PartialEq,
)]
pub enum ChangeKind {
    Direct,
    Propagated,
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
pub struct SubscriptionFilterInput {
    pub actions: Option<Vec<ChangeAction>>,
    pub dummy: Option<bool>,
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
/// Offset pagination input shared by generated list, relation, and search APIs.
///
/// Generated connection resolvers combine this request input with
/// [`PaginationConfig`] from the runtime `Database`. The default config applies
/// a limit of `1000` when this input or its `limit` field is omitted.
pub struct PageInput {
    /// Requested page size. Explicit limits are clamped by the runtime
    /// [`PaginationConfig`] before SQL rendering.
    pub limit: Option<i64>,
    /// Requested offset. Negative offsets are treated as `0`.
    pub offset: Option<i64>,
}

impl PageInput {
    /// Default max limit retained for compatibility with older callers.
    pub const MAX_LIMIT: i64 = PaginationConfig::DEFAULT_MAX_LIMIT;

    /// Return a non-negative offset for SQL rendering and in-memory slicing.
    pub fn offset(&self) -> i64 {
        self.offset.unwrap_or(0).max(0)
    }

    /// Return the explicit requested limit clamped by the default pagination
    /// config. This does not apply the default connection limit when the field
    /// is omitted.
    ///
    /// This compatibility helper cannot observe a `Database` handle's custom
    /// pagination configuration. Use [`Self::limit_with_config`] or
    /// [`PaginationConfig::resolve_page`] for application code that supports
    /// configured caps.
    #[deprecated(
        since = "0.2.17",
        note = "use PageInput::limit_with_config or PaginationConfig::resolve_page"
    )]
    pub fn limit(&self) -> Option<i64> {
        PaginationConfig::default().clamp_explicit_limit(self.limit)
    }

    /// Resolve this input with a caller-provided pagination config.
    pub fn limit_with_config(&self, config: PaginationConfig) -> Option<i64> {
        config.clamp_explicit_limit(self.limit)
    }
}

pub trait PoolProvider<B: OrmBackend = DefaultBackend> {
    fn pool(&self) -> &B::Pool;

    fn pagination_config(&self) -> PaginationConfig {
        PaginationConfig::default()
    }
}

pub trait DatabaseExecutor<B: OrmBackend = DefaultBackend>: PoolProvider<B> {
    fn backend(&self) -> DatabaseBackend {
        B::DIALECT
    }
}

#[cfg(feature = "sqlite")]
impl PoolProvider<SqliteBackend> for sqlx::SqlitePool {
    fn pool(&self) -> &<SqliteBackend as OrmBackend>::Pool {
        self
    }
}

#[cfg(feature = "sqlite")]
impl DatabaseExecutor<SqliteBackend> for sqlx::SqlitePool {}

#[cfg(feature = "postgres")]
impl PoolProvider<PostgresBackend> for sqlx::PgPool {
    fn pool(&self) -> &<PostgresBackend as OrmBackend>::Pool {
        self
    }
}

#[cfg(feature = "postgres")]
impl DatabaseExecutor<PostgresBackend> for sqlx::PgPool {}

#[cfg(feature = "mssql")]
impl PoolProvider<MssqlBackend> for crate::db::mssql::MssqlPool {
    fn pool(&self) -> &<MssqlBackend as OrmBackend>::Pool {
        self
    }
}

#[cfg(feature = "mssql")]
impl DatabaseExecutor<MssqlBackend> for crate::db::mssql::MssqlPool {}

impl<B: OrmBackend> PoolProvider<B> for crate::db::Database<B> {
    fn pool(&self) -> &B::Pool {
        self.pool()
    }

    fn pagination_config(&self) -> PaginationConfig {
        crate::db::Database::pagination_config(self)
    }
}

impl<B: OrmBackend> DatabaseExecutor<B> for crate::db::Database<B> {}

#[allow(async_fn_in_trait)]
pub trait RelationLoader<B: OrmBackend = DefaultBackend> {
    async fn load_relations(
        &mut self,
        pool: &B::Pool,
        selection: &[async_graphql::context::SelectionField<'_>],
    ) -> Result<(), sqlx::Error>;

    async fn bulk_load_relations(
        entities: &mut [Self],
        pool: &B::Pool,
        selection: &[async_graphql::context::SelectionField<'_>],
    ) -> Result<(), sqlx::Error>
    where
        Self: Sized;

    async fn load_relations_with_auth(
        &mut self,
        pool: &B::Pool,
        selection: &[async_graphql::context::SelectionField<'_>],
        _auth: Option<&DbAuthContext>,
    ) -> Result<(), sqlx::Error> {
        self.load_relations(pool, selection).await
    }

    async fn bulk_load_relations_with_auth(
        entities: &mut [Self],
        pool: &B::Pool,
        selection: &[async_graphql::context::SelectionField<'_>],
        _auth: Option<&DbAuthContext>,
    ) -> Result<(), sqlx::Error>
    where
        Self: Sized,
    {
        Self::bulk_load_relations(entities, pool, selection).await
    }
}

/// Small deterministic substring scorer used by fallback query paths.
pub struct FuzzyMatcher {
    query: String,
    threshold: f64,
}

#[derive(Clone, Debug)]
pub struct MatchResult<T> {
    pub entity: T,
    pub score: f64,
}

impl FuzzyMatcher {
    pub fn new(query: &str) -> Self {
        Self {
            query: query.to_lowercase(),
            threshold: 0.0,
        }
    }

    pub fn with_threshold(mut self, threshold: f64) -> Self {
        self.threshold = threshold;
        self
    }

    pub fn filter_and_score<T, F>(&self, items: Vec<T>, extract: F) -> Vec<MatchResult<T>>
    where
        F: Fn(&T) -> Option<&str>,
    {
        let mut out = Vec::new();
        for item in items {
            let score = extract(&item)
                .map(|candidate| {
                    if candidate.to_lowercase().contains(&self.query) {
                        1.0
                    } else {
                        0.0
                    }
                })
                .unwrap_or(0.0);

            if score >= self.threshold {
                out.push(MatchResult {
                    entity: item,
                    score,
                });
            }
        }
        out
    }
}

/// Escape `%`, `_`, and `\` in a user string before binding it to a SQL
/// `LIKE`/`ILIKE` predicate that uses `ESCAPE '\'`.
pub fn escape_like_pattern(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' | '%' | '_' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    escaped
}

/// Build an escaped contains pattern for SQL `LIKE`/`ILIKE`.
pub fn contains_like_pattern(value: &str) -> String {
    format!("%{}%", escape_like_pattern(value))
}

/// Build an escaped starts-with pattern for SQL `LIKE`/`ILIKE`.
pub fn starts_with_like_pattern(value: &str) -> String {
    format!("{}%", escape_like_pattern(value))
}

/// Build an escaped ends-with pattern for SQL `LIKE`/`ILIKE`.
pub fn ends_with_like_pattern(value: &str) -> String {
    format!("%{}", escape_like_pattern(value))
}

pub fn generate_candidate_pattern(value: &str) -> String {
    contains_like_pattern(value)
}

fn filter_expression_from_raw_parts(
    conditions: &[String],
    values: &[SqlValue],
) -> Option<FilterExpression> {
    if conditions.is_empty() {
        return None;
    }

    let mut value_iter = values.iter().cloned();
    let filters = conditions
        .iter()
        .map(|clause| {
            let placeholder_count = count_placeholders(clause);
            let clause_values = value_iter
                .by_ref()
                .take(placeholder_count)
                .collect::<Vec<_>>();
            FilterExpression::Raw {
                clause: clause.clone(),
                values: clause_values,
            }
        })
        .collect::<Vec<_>>();

    if filters.len() == 1 {
        filters.into_iter().next()
    } else {
        Some(FilterExpression::And(filters))
    }
}

pub fn count_placeholders(clause: &str) -> usize {
    let chars: Vec<char> = clause.chars().collect();
    let mut count = 0usize;
    let mut i = 0usize;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut in_bracket_quote = false;
    while i < chars.len() {
        let ch = chars[i];
        if in_single_quote {
            if ch == '\'' {
                if i + 1 < chars.len() && chars[i + 1] == '\'' {
                    i += 2;
                    continue;
                }
                in_single_quote = false;
            }
            i += 1;
            continue;
        }
        if in_double_quote {
            if ch == '"' {
                if i + 1 < chars.len() && chars[i + 1] == '"' {
                    i += 2;
                    continue;
                }
                in_double_quote = false;
            }
            i += 1;
            continue;
        }
        if in_bracket_quote {
            if ch == ']' {
                if i + 1 < chars.len() && chars[i + 1] == ']' {
                    i += 2;
                    continue;
                }
                in_bracket_quote = false;
            }
            i += 1;
            continue;
        }
        match ch {
            '\'' => {
                in_single_quote = true;
                i += 1;
            }
            '"' => {
                in_double_quote = true;
                i += 1;
            }
            '[' => {
                in_bracket_quote = true;
                i += 1;
            }
            '?' => {
                count += 1;
                i += 1;
            }
            '$' => {
                let mut j = i + 1;
                let mut saw_digit = false;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    saw_digit = true;
                    j += 1;
                }
                if saw_digit {
                    count += 1;
                    i = j;
                } else {
                    i += 1;
                }
            }
            '@' if i + 1 < chars.len() && chars[i + 1].eq_ignore_ascii_case(&'p') => {
                let mut j = i + 2;
                let mut saw_digit = false;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    saw_digit = true;
                    j += 1;
                }
                if saw_digit {
                    count += 1;
                    i = j;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    count
}

pub fn render_filter_expression(
    dialect: DatabaseBackend,
    filter: &FilterExpression,
    next_index: &mut usize,
    bind_values: &mut Vec<SqlValue>,
) -> String {
    match filter {
        FilterExpression::Raw { clause, values } => {
            let rendered = dialect.normalize_sql(clause, *next_index);
            *next_index += values.len();
            bind_values.extend(values.iter().cloned());
            rendered
        }
        FilterExpression::And(filters) => filters
            .iter()
            .map(|filter| render_filter_expression(dialect, filter, next_index, bind_values))
            .filter(|sql| !sql.is_empty())
            .map(|sql| format!("({sql})"))
            .collect::<Vec<_>>()
            .join(" AND "),
        FilterExpression::Or(filters) => filters
            .iter()
            .map(|filter| render_filter_expression(dialect, filter, next_index, bind_values))
            .filter(|sql| !sql.is_empty())
            .map(|sql| format!("({sql})"))
            .collect::<Vec<_>>()
            .join(" OR "),
    }
}

pub fn render_select_query(dialect: DatabaseBackend, query: &SelectQuery) -> RenderedQuery {
    let projection = if query.count_only {
        dialect.count_projection().to_string()
    } else {
        query.columns.join(", ")
    };
    let mut sql = format!("SELECT {} FROM {}", projection, query.table);
    let mut values = Vec::new();
    let mut next_index = 1usize;

    if let Some(filter) = &query.filter {
        let where_sql = render_filter_expression(dialect, filter, &mut next_index, &mut values);
        if !where_sql.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&where_sql);
        }
    }

    if !query.count_only && !query.sorts.is_empty() {
        sql.push_str(" ORDER BY ");
        sql.push_str(
            &query
                .sorts
                .iter()
                .map(|sort| sort.clause.clone())
                .collect::<Vec<_>>()
                .join(", "),
        );
    }

    if !query.count_only {
        if let Some(page) = &query.pagination {
            if dialect == DatabaseBackend::Mssql && query.sorts.is_empty() {
                sql.push_str(" ORDER BY (SELECT 1)");
            }
            sql.push_str(&dialect.render_pagination(page.limit, page.offset));
        }
    }

    RenderedQuery { sql, values }
}

pub fn render_aggregate_query(dialect: DatabaseBackend, query: &AggregateQuery) -> RenderedQuery {
    let argument = query.column.as_deref().unwrap_or("*");
    let projection = format!(
        "{}({}) AS __gom_aggregate",
        query.function.as_sql(),
        argument
    );
    let mut sql = format!("SELECT {} FROM {}", projection, query.table);
    let mut values = Vec::new();
    let mut next_index = 1usize;

    if let Some(filter) = &query.filter {
        let where_sql = render_filter_expression(dialect, filter, &mut next_index, &mut values);
        if !where_sql.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&where_sql);
        }
    }

    RenderedQuery { sql, values }
}

pub fn render_delete_query(dialect: DatabaseBackend, query: &DeleteQuery) -> RenderedQuery {
    let mut sql = format!("DELETE FROM {}", query.table);
    let mut values = Vec::new();
    let mut next_index = 1usize;

    if let Some(filter) = &query.filter {
        let where_sql = render_filter_expression(dialect, filter, &mut next_index, &mut values);
        if !where_sql.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&where_sql);
        }
    }

    RenderedQuery { sql, values }
}

pub fn render_upsert_sql(
    dialect: DatabaseBackend,
    table: &str,
    insert_columns: &[&str],
    insert_values: &[&str],
    conflict_columns: &[&str],
    update_columns: &[&str],
    update_updated_at: bool,
) -> String {
    let mut set_clauses = update_columns
        .iter()
        .map(|column| format!("{column} = EXCLUDED.{column}"))
        .collect::<Vec<_>>();
    if update_updated_at {
        set_clauses.push(format!("updated_at = {}", dialect.current_epoch_expr()));
    }

    format!(
        "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) DO UPDATE SET {}",
        table,
        insert_columns.join(", "),
        insert_values.join(", "),
        conflict_columns.join(", "),
        set_clauses.join(", ")
    )
}

pub fn backend_placeholder(index: usize) -> String {
    current_backend().placeholder(index)
}

pub fn normalize_sql(sql: &str, start_index: usize) -> String {
    current_backend().normalize_sql(sql, start_index)
}

pub fn build_upsert_sql(
    table: &str,
    insert_columns: &[&str],
    insert_values: &[&str],
    conflict_columns: &[&str],
    update_columns: &[&str],
    update_updated_at: bool,
) -> String {
    render_upsert_sql(
        current_backend(),
        table,
        insert_columns,
        insert_values,
        conflict_columns,
        update_columns,
        update_updated_at,
    )
}
type EntityMatcher<T> = Arc<dyn Fn(&T) -> Result<bool, sqlx::Error> + Send + Sync>;

struct PoolRef<'a, B: OrmBackend> {
    pool: &'a B::Pool,
}

impl<B: OrmBackend> PoolProvider<B> for PoolRef<'_, B> {
    fn pool(&self) -> &B::Pool {
        self.pool
    }
}

pub struct EntityQuery<T, B: OrmBackend = DefaultBackend> {
    pub where_clauses: Vec<String>,
    pub values: Vec<SqlValue>,
    pub order_clauses: Vec<String>,
    pub page: Option<PageInput>,
    entity_matchers: Vec<EntityMatcher<T>>,
    _marker: PhantomData<(T, B)>,
}

impl<T, B: OrmBackend> Clone for EntityQuery<T, B> {
    fn clone(&self) -> Self {
        Self {
            where_clauses: self.where_clauses.clone(),
            values: self.values.clone(),
            order_clauses: self.order_clauses.clone(),
            page: self.page.clone(),
            entity_matchers: self.entity_matchers.clone(),
            _marker: PhantomData,
        }
    }
}

impl<T, B> EntityQuery<T, B>
where
    B: OrmBackend,
    T: DatabaseEntity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    pub fn new() -> Self {
        Self {
            where_clauses: Vec::new(),
            values: Vec::new(),
            order_clauses: Vec::new(),
            page: None,
            entity_matchers: Vec::new(),
            _marker: PhantomData,
        }
    }

    pub fn where_clause(mut self, clause: &str, value: SqlValue) -> Self {
        self.where_clauses.push(clause.to_string());
        self.values.push(value);
        self
    }

    pub fn where_values(mut self, clause: &str, values: Vec<SqlValue>) -> Self {
        self.where_clauses.push(clause.to_string());
        self.values.extend(values);
        self
    }

    pub fn filter<F>(mut self, filter: &F) -> Self
    where
        F: DatabaseFilter,
    {
        let (conds, values) = filter.to_sql_conditions();
        self.where_clauses.extend(conds);
        self.values.extend(values);
        self
    }

    pub fn filter_with_entity_matching<F>(mut self, filter: &F) -> Self
    where
        F: DatabaseFilter + Clone + Send + Sync + 'static,
    {
        if filter.requires_in_memory_filtering(B::DIALECT) {
            let (conds, values) = filter.to_sql_prefilter_conditions(B::DIALECT);
            self.where_clauses.extend(conds);
            self.values.extend(values);
            let filter = filter.clone();
            self.entity_matchers
                .push(Arc::new(move |entity| filter.matches_entity(entity)));
        } else {
            let (conds, values) = filter.to_sql_conditions();
            self.where_clauses.extend(conds);
            self.values.extend(values);
        }
        self
    }

    pub fn order_by<O>(mut self, order: &O) -> Self
    where
        O: DatabaseOrderBy,
    {
        if let Some(sort) = order.to_sort_expression() {
            self.order_clauses.push(sort.clause);
        }
        self
    }

    pub fn default_order(mut self) -> Self {
        self.order_clauses.push(T::DEFAULT_SORT.to_string());
        self
    }

    pub fn paginate(mut self, page: &PageInput) -> Self {
        self.page = Some(page.clone());
        self
    }

    fn build_select_query_with_config(
        &self,
        pagination_config: PaginationConfig,
        apply_default_limit: bool,
    ) -> SelectQuery {
        let page = pagination_config.resolve_page(self.page.as_ref(), apply_default_limit);
        SelectQuery {
            table: T::TABLE_NAME,
            columns: T::column_names()
                .iter()
                .map(|column| (*column).to_string())
                .collect(),
            filter: filter_expression_from_raw_parts(&self.where_clauses, &self.values),
            sorts: self
                .order_clauses
                .iter()
                .cloned()
                .map(|clause| SortExpression { clause })
                .collect(),
            pagination: if self.requires_in_memory_filtering() {
                None
            } else if page.limit.is_some() || page.offset > 0 {
                Some(page)
            } else {
                None
            },
            count_only: false,
        }
    }

    fn build_select_query(&self) -> SelectQuery {
        self.build_select_query_with_config(PaginationConfig::default(), false)
    }

    fn aggregate_column_sql(column: &str) -> Result<String, sqlx::Error>
    where
        T: DatabaseSchema,
    {
        let column_def = T::columns()
            .iter()
            .find(|definition| definition.name == column || definition.rust_name == column)
            .ok_or_else(|| sqlx::Error::ColumnNotFound(column.to_string()))?;
        Ok(B::DIALECT.quote_identifier_path(column_def.name))
    }

    fn build_aggregate_query(
        &self,
        function: AggregateFunction,
        column: Option<&str>,
    ) -> Result<AggregateQuery, sqlx::Error>
    where
        T: DatabaseSchema,
    {
        if self.requires_in_memory_filtering() {
            return Err(sqlx::Error::Protocol(
                "aggregate queries require filters that can be rendered to SQL".to_string(),
            ));
        }
        let column = column.map(Self::aggregate_column_sql).transpose()?;
        Ok(AggregateQuery {
            table: T::TABLE_NAME,
            function,
            column,
            filter: filter_expression_from_raw_parts(&self.where_clauses, &self.values),
        })
    }

    pub(crate) fn requires_in_memory_filtering(&self) -> bool {
        !self.entity_matchers.is_empty()
    }

    fn matches_entity(&self, entity: &T) -> Result<bool, sqlx::Error> {
        self.entity_matchers
            .iter()
            .try_fold(
                true,
                |matches, matcher| {
                    if matches { matcher(entity) } else { Ok(false) }
                },
            )
    }

    fn apply_in_memory_filtering(
        &self,
        rows: Vec<B::Row>,
        pagination_config: PaginationConfig,
        apply_default_limit: bool,
    ) -> Result<Vec<T>, sqlx::Error> {
        let mut entities = rows
            .iter()
            .map(T::from_row)
            .collect::<Result<Vec<_>, _>>()?;

        if self.requires_in_memory_filtering() {
            entities = entities
                .into_iter()
                .filter_map(|entity| match self.matches_entity(&entity) {
                    Ok(true) => Some(Ok(entity)),
                    Ok(false) => None,
                    Err(error) => Some(Err(error)),
                })
                .collect::<Result<Vec<_>, _>>()?;

            let page = pagination_config.resolve_page(self.page.as_ref(), apply_default_limit);
            if page.limit.is_some() || page.offset > 0 {
                let offset = page.offset.max(0) as usize;
                let limit = page.limit.map(|limit| limit.max(0) as usize);
                let iter = entities.into_iter().skip(offset);
                entities = match limit {
                    Some(limit) => iter.take(limit).collect(),
                    None => iter.collect(),
                };
            }
        }

        Ok(entities)
    }

    async fn fetch_unpaged_filtered<P>(&self, provider: &P) -> Result<Vec<T>, sqlx::Error>
    where
        P: PoolProvider<B> + ?Sized,
    {
        let mut query = self.clone();
        query.page = None;
        query.fetch_all(provider).await
    }

    async fn fetch_unpaged_filtered_with_auth<P>(
        &self,
        provider: &P,
        auth: Option<&DbAuthContext>,
    ) -> Result<Vec<T>, sqlx::Error>
    where
        P: PoolProvider<B> + ?Sized,
    {
        let mut query = self.clone();
        query.page = None;
        query.fetch_all_with_auth(provider, auth).await
    }

    pub async fn fetch_all<P>(&self, provider: &P) -> Result<Vec<T>, sqlx::Error>
    where
        P: PoolProvider<B> + ?Sized,
    {
        let pagination_config = provider.pagination_config();
        let rendered = render_select_query(
            B::DIALECT,
            &self.build_select_query_with_config(pagination_config, false),
        );
        let rows = B::fetch_rows(provider.pool(), &rendered.sql, &rendered.values).await?;
        self.apply_in_memory_filtering(rows, pagination_config, false)
    }

    pub async fn fetch_all_with_auth<P>(
        &self,
        provider: &P,
        auth: Option<&DbAuthContext>,
    ) -> Result<Vec<T>, sqlx::Error>
    where
        P: PoolProvider<B> + ?Sized,
    {
        let pagination_config = provider.pagination_config();
        let rendered = render_select_query(
            B::DIALECT,
            &self.build_select_query_with_config(pagination_config, false),
        );
        let rows =
            B::fetch_rows_with_auth(provider.pool(), &rendered.sql, &rendered.values, auth).await?;
        self.apply_in_memory_filtering(rows, pagination_config, false)
    }

    pub async fn fetch_all_on<'e, E>(&self, executor: E) -> Result<Vec<T>, sqlx::Error>
    where
        B: SqlxBackend,
        E: sqlx::Executor<'e, Database = <B as SqlxBackend>::Database> + Send + 'e,
    {
        let rendered = render_select_query(B::DIALECT, &self.build_select_query());
        let rows = B::fetch_rows_on(executor, rendered.sql, rendered.values).await?;
        self.apply_in_memory_filtering(rows, PaginationConfig::default(), false)
    }

    pub async fn fetch_one<P>(&self, provider: &P) -> Result<Option<T>, sqlx::Error>
    where
        P: PoolProvider<B> + ?Sized,
    {
        Ok(self.fetch_all(provider).await?.into_iter().next())
    }

    pub async fn fetch_one_with_auth<P>(
        &self,
        provider: &P,
        auth: Option<&DbAuthContext>,
    ) -> Result<Option<T>, sqlx::Error>
    where
        P: PoolProvider<B> + ?Sized,
    {
        Ok(self
            .fetch_all_with_auth(provider, auth)
            .await?
            .into_iter()
            .next())
    }

    pub async fn fetch_one_on<'e, E>(&self, executor: E) -> Result<Option<T>, sqlx::Error>
    where
        B: SqlxBackend,
        E: sqlx::Executor<'e, Database = <B as SqlxBackend>::Database> + Send + 'e,
    {
        Ok(self.fetch_all_on(executor).await?.into_iter().next())
    }

    pub async fn count<P>(&self, provider: &P) -> Result<i64, sqlx::Error>
    where
        P: PoolProvider<B> + ?Sized,
    {
        if self.requires_in_memory_filtering() {
            return Ok(self.fetch_unpaged_filtered(provider).await?.len() as i64);
        }
        let mut query = self.build_select_query();
        query.count_only = true;
        query.pagination = None;
        query.sorts.clear();
        let rendered = render_select_query(B::DIALECT, &query);
        let rows = B::fetch_rows(provider.pool(), &rendered.sql, &rendered.values).await?;
        let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
        B::try_get_i64(row, "count")
    }

    pub async fn count_column<P>(&self, provider: &P, column: &str) -> Result<i64, sqlx::Error>
    where
        P: PoolProvider<B> + ?Sized,
        T: DatabaseSchema,
    {
        let rendered = render_aggregate_query(
            B::DIALECT,
            &self.build_aggregate_query(AggregateFunction::Count, Some(column))?,
        );
        let rows = B::fetch_rows(provider.pool(), &rendered.sql, &rendered.values).await?;
        let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
        B::try_get_i64(row, "__gom_aggregate")
    }

    pub async fn max_i64<P>(&self, provider: &P, column: &str) -> Result<Option<i64>, sqlx::Error>
    where
        P: PoolProvider<B> + ?Sized,
        T: DatabaseSchema,
    {
        self.aggregate_optional_i64(provider, AggregateFunction::Max, column)
            .await
    }

    pub async fn min_i64<P>(&self, provider: &P, column: &str) -> Result<Option<i64>, sqlx::Error>
    where
        P: PoolProvider<B> + ?Sized,
        T: DatabaseSchema,
    {
        self.aggregate_optional_i64(provider, AggregateFunction::Min, column)
            .await
    }

    pub async fn max_f64<P>(&self, provider: &P, column: &str) -> Result<Option<f64>, sqlx::Error>
    where
        P: PoolProvider<B> + ?Sized,
        T: DatabaseSchema,
    {
        self.aggregate_optional_f64(provider, AggregateFunction::Max, column)
            .await
    }

    pub async fn min_f64<P>(&self, provider: &P, column: &str) -> Result<Option<f64>, sqlx::Error>
    where
        P: PoolProvider<B> + ?Sized,
        T: DatabaseSchema,
    {
        self.aggregate_optional_f64(provider, AggregateFunction::Min, column)
            .await
    }

    async fn aggregate_optional_i64<P>(
        &self,
        provider: &P,
        function: AggregateFunction,
        column: &str,
    ) -> Result<Option<i64>, sqlx::Error>
    where
        P: PoolProvider<B> + ?Sized,
        T: DatabaseSchema,
    {
        let rendered = render_aggregate_query(
            B::DIALECT,
            &self.build_aggregate_query(function, Some(column))?,
        );
        let rows = B::fetch_rows(provider.pool(), &rendered.sql, &rendered.values).await?;
        let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
        B::try_get_optional_i64(row, "__gom_aggregate")
    }

    async fn aggregate_optional_f64<P>(
        &self,
        provider: &P,
        function: AggregateFunction,
        column: &str,
    ) -> Result<Option<f64>, sqlx::Error>
    where
        P: PoolProvider<B> + ?Sized,
        T: DatabaseSchema,
    {
        let rendered = render_aggregate_query(
            B::DIALECT,
            &self.build_aggregate_query(function, Some(column))?,
        );
        let rows = B::fetch_rows(provider.pool(), &rendered.sql, &rendered.values).await?;
        let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
        B::try_get_optional_f64(row, "__gom_aggregate")
    }

    pub async fn count_with_auth<P>(
        &self,
        provider: &P,
        auth: Option<&DbAuthContext>,
    ) -> Result<i64, sqlx::Error>
    where
        P: PoolProvider<B> + ?Sized,
    {
        if self.requires_in_memory_filtering() {
            return Ok(self
                .fetch_unpaged_filtered_with_auth(provider, auth)
                .await?
                .len() as i64);
        }
        let mut query = self.build_select_query();
        query.count_only = true;
        query.pagination = None;
        query.sorts.clear();
        let rendered = render_select_query(B::DIALECT, &query);
        let rows =
            B::fetch_rows_with_auth(provider.pool(), &rendered.sql, &rendered.values, auth).await?;
        let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
        B::try_get_i64(row, "count")
    }

    pub async fn count_column_with_auth<P>(
        &self,
        provider: &P,
        auth: Option<&DbAuthContext>,
        column: &str,
    ) -> Result<i64, sqlx::Error>
    where
        P: PoolProvider<B> + ?Sized,
        T: DatabaseSchema,
    {
        let rendered = render_aggregate_query(
            B::DIALECT,
            &self.build_aggregate_query(AggregateFunction::Count, Some(column))?,
        );
        let rows =
            B::fetch_rows_with_auth(provider.pool(), &rendered.sql, &rendered.values, auth).await?;
        let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
        B::try_get_i64(row, "__gom_aggregate")
    }

    pub async fn max_i64_with_auth<P>(
        &self,
        provider: &P,
        auth: Option<&DbAuthContext>,
        column: &str,
    ) -> Result<Option<i64>, sqlx::Error>
    where
        P: PoolProvider<B> + ?Sized,
        T: DatabaseSchema,
    {
        self.aggregate_optional_i64_with_auth(provider, auth, AggregateFunction::Max, column)
            .await
    }

    pub async fn min_i64_with_auth<P>(
        &self,
        provider: &P,
        auth: Option<&DbAuthContext>,
        column: &str,
    ) -> Result<Option<i64>, sqlx::Error>
    where
        P: PoolProvider<B> + ?Sized,
        T: DatabaseSchema,
    {
        self.aggregate_optional_i64_with_auth(provider, auth, AggregateFunction::Min, column)
            .await
    }

    async fn aggregate_optional_i64_with_auth<P>(
        &self,
        provider: &P,
        auth: Option<&DbAuthContext>,
        function: AggregateFunction,
        column: &str,
    ) -> Result<Option<i64>, sqlx::Error>
    where
        P: PoolProvider<B> + ?Sized,
        T: DatabaseSchema,
    {
        let rendered = render_aggregate_query(
            B::DIALECT,
            &self.build_aggregate_query(function, Some(column))?,
        );
        let rows =
            B::fetch_rows_with_auth(provider.pool(), &rendered.sql, &rendered.values, auth).await?;
        let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
        B::try_get_optional_i64(row, "__gom_aggregate")
    }

    pub async fn count_on<'e, E>(&self, executor: E) -> Result<i64, sqlx::Error>
    where
        B: SqlxBackend,
        E: sqlx::Executor<'e, Database = <B as SqlxBackend>::Database> + Send + 'e,
    {
        if self.requires_in_memory_filtering() {
            let mut query = self.clone();
            query.page = None;
            return Ok(query.fetch_all_on(executor).await?.len() as i64);
        }
        let mut query = self.build_select_query();
        query.count_only = true;
        query.pagination = None;
        query.sorts.clear();
        let rendered = render_select_query(B::DIALECT, &query);
        let rows = B::fetch_rows_on(executor, rendered.sql, rendered.values).await?;
        let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
        B::try_get_i64(row, "count")
    }

    pub fn build_delete_sql(&self) -> (String, Vec<SqlValue>) {
        let rendered = render_delete_query(
            B::DIALECT,
            &DeleteQuery {
                table: T::TABLE_NAME,
                filter: filter_expression_from_raw_parts(&self.where_clauses, &self.values),
            },
        );
        (rendered.sql, rendered.values)
    }

    pub async fn fetch_connection<P>(&self, provider: &P) -> Result<Connection<T>, sqlx::Error>
    where
        P: PoolProvider<B> + ?Sized,
    {
        if self.requires_in_memory_filtering() {
            let nodes = self.fetch_unpaged_filtered(provider).await?;
            let total = nodes.len() as i64;
            let page = provider
                .pagination_config()
                .resolve_page(self.page.as_ref(), true);
            let offset = page.offset.max(0) as usize;
            let limit = page.limit.map(|limit| limit.max(0) as usize);
            let page_nodes = if offset >= nodes.len() {
                Vec::new()
            } else {
                let iter = nodes.into_iter().skip(offset);
                match limit {
                    Some(limit) => iter.take(limit).collect(),
                    None => iter.collect(),
                }
            };
            let edges = page_nodes
                .into_iter()
                .enumerate()
                .map(|(index, node)| Edge {
                    node,
                    cursor: encode_cursor((offset + index) as i64),
                })
                .collect::<Vec<_>>();

            return Ok(Connection {
                page_info: PageInfo {
                    has_next_page: (offset as i64 + edges.len() as i64) < total,
                    has_previous_page: offset > 0,
                    start_cursor: edges.first().map(|edge| edge.cursor.clone()),
                    end_cursor: edges.last().map(|edge| edge.cursor.clone()),
                    total_count: Some(total),
                },
                edges,
            });
        }
        let pagination_config = provider.pagination_config();
        let page = pagination_config.resolve_page(self.page.as_ref(), true);
        let offset = page.offset.max(0) as usize;
        let mut count_query = self.build_select_query();
        count_query.count_only = true;
        count_query.pagination = None;
        count_query.sorts.clear();
        let count_rendered = render_select_query(B::DIALECT, &count_query);
        let row_rendered = render_select_query(
            B::DIALECT,
            &self.build_select_query_with_config(pagination_config, true),
        );
        let (count_rows, rows) = B::fetch_rows_pair_with_auth(
            provider.pool(),
            &count_rendered.sql,
            &count_rendered.values,
            &row_rendered.sql,
            &row_rendered.values,
            None,
        )
        .await?;
        let count_row = count_rows.first().ok_or(sqlx::Error::RowNotFound)?;
        let total = B::try_get_i64(count_row, "count")?;
        let nodes = rows
            .iter()
            .map(T::from_row)
            .collect::<Result<Vec<_>, _>>()?;
        let edges = nodes
            .into_iter()
            .enumerate()
            .map(|(index, node)| Edge {
                node,
                cursor: encode_cursor((offset + index) as i64),
            })
            .collect::<Vec<_>>();

        Ok(Connection {
            page_info: PageInfo {
                has_next_page: (offset as i64 + edges.len() as i64) < total,
                has_previous_page: offset > 0,
                start_cursor: edges.first().map(|edge| edge.cursor.clone()),
                end_cursor: edges.last().map(|edge| edge.cursor.clone()),
                total_count: Some(total),
            },
            edges,
        })
    }

    pub async fn fetch_connection_with_auth<P>(
        &self,
        provider: &P,
        auth: Option<&DbAuthContext>,
    ) -> Result<Connection<T>, sqlx::Error>
    where
        P: PoolProvider<B> + ?Sized,
    {
        if self.requires_in_memory_filtering() {
            let nodes = self
                .fetch_unpaged_filtered_with_auth(provider, auth)
                .await?;
            let total = nodes.len() as i64;
            let page = provider
                .pagination_config()
                .resolve_page(self.page.as_ref(), true);
            let offset = page.offset.max(0) as usize;
            let limit = page.limit.map(|limit| limit.max(0) as usize);
            let page_nodes = if offset >= nodes.len() {
                Vec::new()
            } else {
                let iter = nodes.into_iter().skip(offset);
                match limit {
                    Some(limit) => iter.take(limit).collect(),
                    None => iter.collect(),
                }
            };
            let edges = page_nodes
                .into_iter()
                .enumerate()
                .map(|(index, node)| Edge {
                    node,
                    cursor: encode_cursor((offset + index) as i64),
                })
                .collect::<Vec<_>>();

            return Ok(Connection {
                page_info: PageInfo {
                    has_next_page: (offset as i64 + edges.len() as i64) < total,
                    has_previous_page: offset > 0,
                    start_cursor: edges.first().map(|edge| edge.cursor.clone()),
                    end_cursor: edges.last().map(|edge| edge.cursor.clone()),
                    total_count: Some(total),
                },
                edges,
            });
        }
        let mut count_query = self.build_select_query();
        count_query.count_only = true;
        count_query.pagination = None;
        count_query.sorts.clear();
        let count_rendered = render_select_query(B::DIALECT, &count_query);
        let pagination_config = provider.pagination_config();
        let row_rendered = render_select_query(
            B::DIALECT,
            &self.build_select_query_with_config(pagination_config, true),
        );
        let (count_rows, rows) = B::fetch_rows_pair_with_auth(
            provider.pool(),
            &count_rendered.sql,
            &count_rendered.values,
            &row_rendered.sql,
            &row_rendered.values,
            auth,
        )
        .await?;
        let count_row = count_rows.first().ok_or(sqlx::Error::RowNotFound)?;
        let total = B::try_get_i64(count_row, "count")?;
        let page = pagination_config.resolve_page(self.page.as_ref(), true);
        let offset = page.offset.max(0) as usize;
        let nodes = rows
            .iter()
            .map(T::from_row)
            .collect::<Result<Vec<_>, _>>()?;
        let edges = nodes
            .into_iter()
            .enumerate()
            .map(|(index, node)| Edge {
                node,
                cursor: encode_cursor((offset + index) as i64),
            })
            .collect::<Vec<_>>();

        Ok(Connection {
            page_info: PageInfo {
                has_next_page: (offset as i64 + edges.len() as i64) < total,
                has_previous_page: offset > 0,
                start_cursor: edges.first().map(|edge| edge.cursor.clone()),
                end_cursor: edges.last().map(|edge| edge.cursor.clone()),
                total_count: Some(total),
            },
            edges,
        })
    }
}

pub struct FindQuery<'a, T, W, O, B: OrmBackend = DefaultBackend>
where
    T: DatabaseEntity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    pool: &'a B::Pool,
    query: EntityQuery<T, B>,
    _marker: PhantomData<(W, O, B)>,
}

impl<'a, T, W, O, B> FindQuery<'a, T, W, O, B>
where
    B: OrmBackend,
    T: DatabaseEntity + FromSqlRow<B> + Clone + Send + Sync + 'static,
    W: DatabaseFilter + Clone + Send + Sync + 'static,
{
    pub fn new(pool: &'a B::Pool) -> Self {
        Self {
            pool,
            query: EntityQuery::new(),
            _marker: PhantomData,
        }
    }

    pub fn filter(mut self, filter: W) -> Self {
        self.query = self.query.filter_with_entity_matching(&filter);
        self
    }

    pub fn order_by(mut self, order: O) -> Self
    where
        O: DatabaseOrderBy,
    {
        self.query = self.query.order_by(&order);
        self
    }

    pub fn limit(mut self, limit: i64) -> Self {
        let mut page = self.query.page.unwrap_or_default();
        page.limit = Some(limit);
        self.query.page = Some(page);
        self
    }

    pub fn offset(mut self, offset: i64) -> Self {
        let mut page = self.query.page.unwrap_or_default();
        page.offset = Some(offset);
        self.query.page = Some(page);
        self
    }

    pub async fn fetch_all(self) -> Result<Vec<T>, sqlx::Error> {
        self.query.fetch_all(&PoolRef { pool: self.pool }).await
    }

    pub async fn fetch_all_on<'e, E>(self, executor: E) -> Result<Vec<T>, sqlx::Error>
    where
        B: SqlxBackend,
        E: sqlx::Executor<'e, Database = <B as SqlxBackend>::Database> + Send + 'e,
    {
        self.query.fetch_all_on(executor).await
    }

    pub async fn fetch_one(self) -> Result<Option<T>, sqlx::Error> {
        Ok(self.fetch_all().await?.into_iter().next())
    }

    pub async fn fetch_one_on<'e, E>(self, executor: E) -> Result<Option<T>, sqlx::Error>
    where
        B: SqlxBackend,
        E: sqlx::Executor<'e, Database = <B as SqlxBackend>::Database> + Send + 'e,
    {
        self.query.fetch_one_on(executor).await
    }

    pub async fn count(self) -> Result<i64, sqlx::Error> {
        self.query.count(&PoolRef { pool: self.pool }).await
    }

    pub async fn count_column(self, column: &str) -> Result<i64, sqlx::Error>
    where
        T: DatabaseSchema,
    {
        self.query
            .count_column(&PoolRef { pool: self.pool }, column)
            .await
    }

    pub async fn max_i64(self, column: &str) -> Result<Option<i64>, sqlx::Error>
    where
        T: DatabaseSchema,
    {
        self.query
            .max_i64(&PoolRef { pool: self.pool }, column)
            .await
    }

    pub async fn min_i64(self, column: &str) -> Result<Option<i64>, sqlx::Error>
    where
        T: DatabaseSchema,
    {
        self.query
            .min_i64(&PoolRef { pool: self.pool }, column)
            .await
    }

    pub async fn max_f64(self, column: &str) -> Result<Option<f64>, sqlx::Error>
    where
        T: DatabaseSchema,
    {
        self.query
            .max_f64(&PoolRef { pool: self.pool }, column)
            .await
    }

    pub async fn min_f64(self, column: &str) -> Result<Option<f64>, sqlx::Error>
    where
        T: DatabaseSchema,
    {
        self.query
            .min_f64(&PoolRef { pool: self.pool }, column)
            .await
    }

    pub async fn count_on<'e, E>(self, executor: E) -> Result<i64, sqlx::Error>
    where
        B: SqlxBackend,
        E: sqlx::Executor<'e, Database = <B as SqlxBackend>::Database> + Send + 'e,
    {
        self.query.count_on(executor).await
    }

    pub async fn exists(self) -> Result<bool, sqlx::Error> {
        Ok(self.count().await? > 0)
    }

    pub async fn exists_on<'e, E>(self, executor: E) -> Result<bool, sqlx::Error>
    where
        B: SqlxBackend,
        E: sqlx::Executor<'e, Database = <B as SqlxBackend>::Database> + Send + 'e,
    {
        Ok(self.query.count_on(executor).await? > 0)
    }
}

pub struct EntityCountQuery<'a, T, W, B: OrmBackend = DefaultBackend>
where
    T: DatabaseEntity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    pool: &'a B::Pool,
    query: EntityQuery<T, B>,
    _marker: PhantomData<W>,
}

impl<'a, T, W, B> EntityCountQuery<'a, T, W, B>
where
    B: OrmBackend,
    T: DatabaseEntity + FromSqlRow<B> + Clone + Send + Sync + 'static,
    W: DatabaseFilter + Clone + Send + Sync + 'static,
{
    pub fn new(pool: &'a B::Pool) -> Self {
        Self {
            pool,
            query: EntityQuery::new(),
            _marker: PhantomData,
        }
    }

    pub fn filter(mut self, filter: &W) -> Self {
        self.query = self.query.filter_with_entity_matching(filter);
        self
    }

    pub async fn count(self) -> Result<i64, sqlx::Error> {
        self.query.count(&PoolRef { pool: self.pool }).await
    }

    pub async fn count_on<'e, E>(self, executor: E) -> Result<i64, sqlx::Error>
    where
        B: SqlxBackend,
        E: sqlx::Executor<'e, Database = <B as SqlxBackend>::Database> + Send + 'e,
    {
        self.query.count_on(executor).await
    }
}

pub struct CountQuery<'a, W, B: OrmBackend = DefaultBackend> {
    pool: &'a B::Pool,
    table: &'static str,
    filters: Vec<String>,
    values: Vec<SqlValue>,
    _marker: PhantomData<(W, B)>,
}

impl<'a, W, B> CountQuery<'a, W, B>
where
    B: OrmBackend,
    W: DatabaseFilter,
{
    pub fn new(pool: &'a B::Pool, table: &'static str) -> Self {
        Self {
            pool,
            table,
            filters: Vec::new(),
            values: Vec::new(),
            _marker: PhantomData,
        }
    }

    pub fn filter(mut self, filter: &W) -> Self {
        let (conds, values) = filter.to_sql_conditions();
        self.filters.extend(conds);
        self.values.extend(values);
        self
    }

    pub async fn count(self) -> Result<i64, sqlx::Error> {
        let rendered = render_select_query(
            B::DIALECT,
            &SelectQuery {
                table: self.table,
                columns: Vec::new(),
                filter: filter_expression_from_raw_parts(&self.filters, &self.values),
                sorts: Vec::new(),
                pagination: None,
                count_only: true,
            },
        );
        let rows = B::fetch_rows(self.pool, &rendered.sql, &rendered.values).await?;
        let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
        B::try_get_i64(row, "count")
    }

    pub async fn count_on<'e, E>(self, executor: E) -> Result<i64, sqlx::Error>
    where
        B: SqlxBackend,
        E: sqlx::Executor<'e, Database = <B as SqlxBackend>::Database> + Send + 'e,
    {
        let rendered = render_select_query(
            B::DIALECT,
            &SelectQuery {
                table: self.table,
                columns: Vec::new(),
                filter: filter_expression_from_raw_parts(&self.filters, &self.values),
                sorts: Vec::new(),
                pagination: None,
                count_only: true,
            },
        );
        let rows = B::fetch_rows_on(executor, rendered.sql, rendered.values).await?;
        let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
        B::try_get_i64(row, "count")
    }
}
