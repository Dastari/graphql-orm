#[cfg(feature = "mssql")]
use super::MssqlBackend;
#[cfg(feature = "postgres")]
use super::PostgresBackend;
#[cfg(feature = "sqlite")]
use super::SqliteBackend;
use super::core::{ColumnDef, EntityMetadata, IndexDef, RelationMetadata, SchemaPolicy, SqlValue};
use super::dialect::{DatabaseBackend, SqlDialect, current_backend};
use super::{DefaultBackend, OrmBackend, SqlxBackend};
use crate::graphql::pagination::{Connection, Edge, PageInfo, encode_cursor};
use std::marker::PhantomData;

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

pub trait Entity: DatabaseEntity + DatabaseSchema + EntityRelations {
    fn entity_name() -> &'static str;
    fn metadata() -> &'static EntityMetadata;
}

pub trait FromSqlRow<B: OrmBackend = DefaultBackend>: Sized {
    fn from_row(row: &B::Row) -> Result<Self, sqlx::Error>;
}

pub trait DatabaseFilter {
    fn to_sql_conditions(&self) -> (Vec<String>, Vec<SqlValue>);
    fn is_empty(&self) -> bool;

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

#[derive(Clone, Debug, PartialEq)]
pub struct DeleteQuery {
    pub table: &'static str,
    pub filter: Option<FilterExpression>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PaginationRequest {
    pub limit: Option<i64>,
    pub offset: i64,
}

impl From<&PageInput> for PaginationRequest {
    fn from(value: &PageInput) -> Self {
        Self {
            limit: value.limit(),
            offset: value.offset(),
        }
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
pub struct PageInput {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

impl PageInput {
    pub fn offset(&self) -> i64 {
        self.offset.unwrap_or(0)
    }

    pub fn limit(&self) -> Option<i64> {
        self.limit
    }
}

pub trait PoolProvider<B: OrmBackend = DefaultBackend> {
    fn pool(&self) -> &B::Pool;
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
}

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

pub fn generate_candidate_pattern(value: &str) -> String {
    format!("%{}%", value)
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

fn count_placeholders(clause: &str) -> usize {
    let chars: Vec<char> = clause.chars().collect();
    let mut count = 0usize;
    let mut i = 0usize;
    while i < chars.len() {
        match chars[i] {
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

fn render_filter_expression(
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
pub struct EntityQuery<T, B: OrmBackend = DefaultBackend> {
    pub where_clauses: Vec<String>,
    pub values: Vec<SqlValue>,
    pub order_clauses: Vec<String>,
    pub page: Option<PageInput>,
    _marker: PhantomData<(T, B)>,
}

impl<T, B: OrmBackend> Clone for EntityQuery<T, B> {
    fn clone(&self) -> Self {
        Self {
            where_clauses: self.where_clauses.clone(),
            values: self.values.clone(),
            order_clauses: self.order_clauses.clone(),
            page: self.page.clone(),
            _marker: PhantomData,
        }
    }
}

impl<T, B> EntityQuery<T, B>
where
    B: OrmBackend,
    T: DatabaseEntity + FromSqlRow<B> + Clone + Send + Sync,
{
    pub fn new() -> Self {
        Self {
            where_clauses: Vec::new(),
            values: Vec::new(),
            order_clauses: Vec::new(),
            page: None,
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

    fn build_select_query(&self) -> SelectQuery {
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
            pagination: self.page.as_ref().map(PaginationRequest::from),
            count_only: false,
        }
    }

    pub async fn fetch_all<P>(&self, provider: &P) -> Result<Vec<T>, sqlx::Error>
    where
        P: PoolProvider<B> + ?Sized,
    {
        let rendered = render_select_query(B::DIALECT, &self.build_select_query());
        let rows = B::fetch_rows(provider.pool(), &rendered.sql, &rendered.values).await?;
        rows.iter().map(T::from_row).collect()
    }

    pub async fn fetch_all_on<'e, E>(&self, executor: E) -> Result<Vec<T>, sqlx::Error>
    where
        B: SqlxBackend,
        E: sqlx::Executor<'e, Database = <B as SqlxBackend>::Database> + Send + 'e,
    {
        let rendered = render_select_query(B::DIALECT, &self.build_select_query());
        let rows = B::fetch_rows_on(executor, rendered.sql, rendered.values).await?;
        rows.iter().map(T::from_row).collect()
    }

    pub async fn fetch_one<P>(&self, provider: &P) -> Result<Option<T>, sqlx::Error>
    where
        P: PoolProvider<B> + ?Sized,
    {
        Ok(self.fetch_all(provider).await?.into_iter().next())
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
        let mut query = self.build_select_query();
        query.count_only = true;
        query.pagination = None;
        query.sorts.clear();
        let rendered = render_select_query(B::DIALECT, &query);
        let rows = B::fetch_rows(provider.pool(), &rendered.sql, &rendered.values).await?;
        let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
        B::try_get_i64(row, "count")
    }

    pub async fn count_on<'e, E>(&self, executor: E) -> Result<i64, sqlx::Error>
    where
        B: SqlxBackend,
        E: sqlx::Executor<'e, Database = <B as SqlxBackend>::Database> + Send + 'e,
    {
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
        let total = self.count(provider).await?;
        let offset = self.page.as_ref().map(|p| p.offset()).unwrap_or(0) as usize;
        let nodes = self.fetch_all(provider).await?;
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
    T: DatabaseEntity + FromSqlRow<B> + Clone + Send + Sync,
{
    pool: &'a B::Pool,
    query: EntityQuery<T, B>,
    _marker: PhantomData<(W, O, B)>,
}

impl<'a, T, W, O, B> FindQuery<'a, T, W, O, B>
where
    B: OrmBackend,
    T: DatabaseEntity + FromSqlRow<B> + Clone + Send + Sync,
{
    pub fn new(pool: &'a B::Pool) -> Self {
        Self {
            pool,
            query: EntityQuery::new(),
            _marker: PhantomData,
        }
    }

    pub fn filter(mut self, filter: W) -> Self
    where
        W: DatabaseFilter,
    {
        self.query = self.query.filter(&filter);
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
        self.query.page = Some(PageInput {
            limit: Some(limit),
            offset: Some(0),
        });
        self
    }

    pub fn offset(mut self, offset: i64) -> Self {
        let mut page = self.query.page.unwrap_or_default();
        page.offset = Some(offset);
        self.query.page = Some(page);
        self
    }

    pub async fn fetch_all(self) -> Result<Vec<T>, sqlx::Error> {
        let rendered = render_select_query(B::DIALECT, &self.query.build_select_query());
        let rows = B::fetch_rows(self.pool, &rendered.sql, &rendered.values).await?;
        rows.iter().map(T::from_row).collect()
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
        let mut query = self.query.build_select_query();
        query.count_only = true;
        query.pagination = None;
        query.sorts.clear();
        let rendered = render_select_query(B::DIALECT, &query);
        let rows = B::fetch_rows(self.pool, &rendered.sql, &rendered.values).await?;
        let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
        B::try_get_i64(row, "count")
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
