use std::collections::HashMap;
use std::marker::PhantomData;

pub use async_graphql;
pub use futures;
pub use graphql_orm_macros::*;
pub use sqlx;
pub use tokio;
pub use tokio_stream;
pub use uuid;

#[cfg(all(feature = "sqlite", feature = "postgres"))]
compile_error!("Enable only one backend feature for graphql-orm.");

#[cfg(not(any(feature = "sqlite", feature = "postgres")))]
compile_error!("Enable exactly one backend feature for graphql-orm.");

#[cfg(feature = "sqlite")]
pub type DbPool = sqlx::SqlitePool;
#[cfg(feature = "sqlite")]
pub type DbRow = sqlx::sqlite::SqliteRow;

#[cfg(feature = "postgres")]
pub type DbPool = sqlx::PgPool;
#[cfg(feature = "postgres")]
pub type DbRow = sqlx::postgres::PgRow;

pub mod db {
    use super::DbPool;

    #[derive(Clone)]
    pub struct Database {
        pool: DbPool,
    }

    impl Database {
        pub fn new(pool: DbPool) -> Self {
            Self { pool }
        }

        pub fn pool(&self) -> &DbPool {
            &self.pool
        }
    }

    #[cfg(feature = "sqlite")]
    pub mod sqlite_helpers {
        pub fn int_to_bool(value: i32) -> bool {
            value != 0
        }

        pub fn str_to_uuid(value: &str) -> Result<String, std::convert::Infallible> {
            Ok(value.to_string())
        }

        pub fn str_to_datetime(value: &str) -> Result<String, std::convert::Infallible> {
            Ok(value.to_string())
        }

        pub fn json_to_vec<T>(value: &str) -> Vec<T>
        where
            T: serde::de::DeserializeOwned,
        {
            serde_json::from_str(value).unwrap_or_default()
        }
    }

    #[cfg(feature = "postgres")]
    pub mod postgres_helpers {
        pub fn int_to_bool(value: i32) -> bool {
            value != 0
        }

        pub fn str_to_uuid(value: &str) -> Result<String, std::convert::Infallible> {
            Ok(value.to_string())
        }

        pub fn str_to_datetime(value: &str) -> Result<String, std::convert::Infallible> {
            Ok(value.to_string())
        }

        pub fn json_to_vec<T>(value: &str) -> Vec<T>
        where
            T: serde::de::DeserializeOwned,
        {
            serde_json::from_str(value).unwrap_or_default()
        }
    }
}

pub mod graphql {
    use super::{DbPool, DbRow, HashMap, PhantomData};

    pub mod auth {
        pub trait AuthExt {
            fn auth_user(&self) -> async_graphql::Result<String>;
        }

        impl AuthExt for async_graphql::Context<'_> {
            fn auth_user(&self) -> async_graphql::Result<String> {
                self.data_opt::<String>()
                    .cloned()
                    .ok_or_else(|| async_graphql::Error::new("missing auth user in GraphQL context"))
            }
        }
    }

    pub mod pagination {
        #[derive(async_graphql::SimpleObject, Clone, Debug, Default)]
        pub struct PageInfo {
            pub has_next_page: bool,
            pub has_previous_page: bool,
            pub start_cursor: Option<String>,
            pub end_cursor: Option<String>,
            pub total_count: Option<i64>,
        }

        #[derive(Clone, Debug)]
        pub struct Edge<T> {
            pub node: T,
            pub cursor: String,
        }

        #[derive(Clone, Debug)]
        pub struct Connection<T> {
            pub edges: Vec<Edge<T>>,
            pub page_info: PageInfo,
        }

        pub fn encode_cursor(offset: i64) -> String {
            offset.to_string()
        }
    }

    pub mod filters {
        #[derive(async_graphql::InputObject, Clone, Debug, Default)]
        pub struct SimilarityInput {
            #[graphql(name = "Value")]
            pub value: String,
        }

        #[derive(async_graphql::InputObject, Clone, Debug, Default)]
        pub struct StringFilter {
            #[graphql(name = "Eq")]
            pub eq: Option<String>,
            #[graphql(name = "Ne")]
            pub ne: Option<String>,
            #[graphql(name = "Contains")]
            pub contains: Option<String>,
            #[graphql(name = "StartsWith")]
            pub starts_with: Option<String>,
            #[graphql(name = "EndsWith")]
            pub ends_with: Option<String>,
            #[graphql(name = "In")]
            pub in_list: Option<Vec<String>>,
            #[graphql(name = "NotIn")]
            pub not_in: Option<Vec<String>>,
            #[graphql(name = "IsNull")]
            pub is_null: Option<bool>,
            #[graphql(name = "Similar")]
            pub similar: Option<SimilarityInput>,
        }

        #[derive(async_graphql::InputObject, Clone, Debug, Default)]
        pub struct IntFilter {
            #[graphql(name = "Eq")]
            pub eq: Option<i32>,
            #[graphql(name = "Ne")]
            pub ne: Option<i32>,
            #[graphql(name = "Lt")]
            pub lt: Option<i32>,
            #[graphql(name = "Lte")]
            pub lte: Option<i32>,
            #[graphql(name = "Gt")]
            pub gt: Option<i32>,
            #[graphql(name = "Gte")]
            pub gte: Option<i32>,
            #[graphql(name = "In")]
            pub in_list: Option<Vec<i32>>,
            #[graphql(name = "NotIn")]
            pub not_in: Option<Vec<i32>>,
            #[graphql(name = "IsNull")]
            pub is_null: Option<bool>,
        }

        #[derive(async_graphql::InputObject, Clone, Debug, Default)]
        pub struct BoolFilter {
            #[graphql(name = "Eq")]
            pub eq: Option<bool>,
            #[graphql(name = "Ne")]
            pub ne: Option<bool>,
            #[graphql(name = "IsNull")]
            pub is_null: Option<bool>,
        }

        #[derive(async_graphql::InputObject, Clone, Debug, Default)]
        pub struct DateRangeInput {
            #[graphql(name = "Start")]
            pub start: Option<String>,
            #[graphql(name = "End")]
            pub end: Option<String>,
        }

        #[derive(async_graphql::InputObject, Clone, Debug, Default)]
        pub struct RelativeDateInput {
            #[graphql(name = "Days")]
            pub days: i32,
        }

        impl RelativeDateInput {
            pub fn to_sql_expr(&self) -> String {
                if cfg!(feature = "postgres") {
                    format!("CURRENT_DATE + INTERVAL '{} days'", self.days)
                } else {
                    format!("date('now', '+{} days')", self.days)
                }
            }
        }

        #[derive(async_graphql::InputObject, Clone, Debug, Default)]
        pub struct DateFilter {
            #[graphql(name = "Eq")]
            pub eq: Option<String>,
            #[graphql(name = "Ne")]
            pub ne: Option<String>,
            #[graphql(name = "Lt")]
            pub lt: Option<String>,
            #[graphql(name = "Lte")]
            pub lte: Option<String>,
            #[graphql(name = "Gt")]
            pub gt: Option<String>,
            #[graphql(name = "Gte")]
            pub gte: Option<String>,
            #[graphql(name = "Between")]
            pub between: Option<DateRangeInput>,
            #[graphql(name = "IsNull")]
            pub is_null: Option<bool>,
            #[graphql(name = "InPast")]
            pub in_past: Option<bool>,
            #[graphql(name = "InFuture")]
            pub in_future: Option<bool>,
            #[graphql(name = "IsToday")]
            pub is_today: Option<bool>,
            #[graphql(name = "RecentDays")]
            pub recent_days: Option<i32>,
            #[graphql(name = "WithinDays")]
            pub within_days: Option<i32>,
            #[graphql(name = "GteRelative")]
            pub gte_relative: Option<RelativeDateInput>,
            #[graphql(name = "LteRelative")]
            pub lte_relative: Option<RelativeDateInput>,
        }
    }

    pub mod orm {
        use super::pagination::{Connection, Edge, PageInfo, encode_cursor};
        use super::{DbPool, DbRow, PhantomData};
        use sqlx::Row;

        #[derive(Clone, Debug, PartialEq)]
        pub enum SqlValue {
            String(String),
            Int(i64),
            Float(f64),
            Bool(bool),
            Null,
        }

        #[derive(Clone, Debug, PartialEq)]
        pub struct ColumnDef {
            pub name: &'static str,
            pub sql_type: &'static str,
            pub nullable: bool,
            pub is_primary_key: bool,
            pub is_unique: bool,
            pub default: Option<&'static str>,
            pub references: Option<&'static str>,
        }

        #[derive(Clone, Debug, PartialEq)]
        pub struct IndexDef {
            pub name: &'static str,
            pub columns: &'static [&'static str],
            pub is_unique: bool,
        }

        impl IndexDef {
            pub fn new(name: &'static str, columns: &'static [&'static str]) -> Self {
                Self {
                    name,
                    columns,
                    is_unique: false,
                }
            }

            pub fn unique(mut self) -> Self {
                self.is_unique = true;
                self
            }
        }

        #[derive(Clone, Debug)]
        pub struct RelationMetadata {
            pub field_name: &'static str,
            pub target_type: &'static str,
            pub is_multiple: bool,
        }

        pub trait DatabaseEntity {
            const TABLE_NAME: &'static str;
            const PLURAL_NAME: &'static str;
            const PRIMARY_KEY: &'static str;
            const DEFAULT_SORT: &'static str;

            fn column_names() -> &'static [&'static str];
        }

        pub trait DatabaseSchema {
            fn columns() -> &'static [ColumnDef];
            fn indexes() -> &'static [IndexDef];
            fn composite_unique_indexes() -> &'static [&'static [&'static str]];
        }

        pub trait FromSqlRow: Sized {
            fn from_row(row: &DbRow) -> Result<Self, sqlx::Error>;
        }

        pub trait DatabaseFilter {
            fn to_sql_conditions(&self) -> (Vec<String>, Vec<SqlValue>);
            fn is_empty(&self) -> bool;
        }

        pub trait DatabaseOrderBy {
            fn to_sql_order(&self) -> Option<String>;
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
            async_graphql::Enum,
            serde::Serialize,
            serde::Deserialize,
            Copy,
            Clone,
            Debug,
            Eq,
            PartialEq,
        )]
        pub enum ChangeAction {
            Created,
            Updated,
            Deleted,
        }

        #[derive(async_graphql::InputObject, Clone, Debug, Default)]
        pub struct SubscriptionFilterInput {
            #[graphql(name = "Dummy")]
            pub dummy: Option<bool>,
        }

        #[derive(async_graphql::InputObject, Clone, Debug, Default)]
        pub struct PageInput {
            #[graphql(name = "Limit")]
            pub limit: Option<i64>,
            #[graphql(name = "Offset")]
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

        pub trait PoolProvider {
            fn pool(&self) -> &DbPool;
        }

        impl PoolProvider for DbPool {
            fn pool(&self) -> &DbPool {
                self
            }
        }

        impl PoolProvider for crate::db::Database {
            fn pool(&self) -> &DbPool {
                self.pool()
            }
        }

        #[allow(async_fn_in_trait)]
        pub trait RelationLoader {
            async fn load_relations(
                &mut self,
                pool: &DbPool,
                selection: &[async_graphql::context::SelectionField<'_>],
            ) -> Result<(), sqlx::Error>;

            async fn bulk_load_relations(
                entities: &mut [Self],
                pool: &DbPool,
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

        pub fn backend_placeholder(index: usize) -> String {
            if cfg!(feature = "postgres") {
                format!("${index}")
            } else {
                "?".to_string()
            }
        }

        pub fn normalize_sql(sql: &str, start_index: usize) -> String {
            if !cfg!(feature = "postgres") {
                return sql.to_string();
            }

            let chars: Vec<char> = sql.chars().collect();
            let mut out = String::with_capacity(sql.len() + 16);
            let mut i = 0usize;
            let mut next = start_index;
            while i < chars.len() {
                if chars[i] == '?' || chars[i] == '$' {
                    out.push_str(&backend_placeholder(next));
                    next += 1;
                    i += 1;
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

        #[cfg(feature = "sqlite")]
        pub async fn execute_with_binds(
            sql: &str,
            values: &[SqlValue],
            pool: &DbPool,
        ) -> Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error> {
            let mut query = sqlx::query(sql);
            for value in values {
                query = match value {
                    SqlValue::String(value) => query.bind(value),
                    SqlValue::Int(value) => query.bind(*value),
                    SqlValue::Float(value) => query.bind(*value),
                    SqlValue::Bool(value) => query.bind(*value),
                    SqlValue::Null => query.bind(Option::<String>::None),
                };
            }
            query.execute(pool).await
        }

        #[cfg(feature = "postgres")]
        pub async fn execute_with_binds(
            sql: &str,
            values: &[SqlValue],
            pool: &DbPool,
        ) -> Result<sqlx::postgres::PgQueryResult, sqlx::Error> {
            let sql = normalize_sql(sql, 1);
            let mut query = sqlx::query(&sql);
            for value in values {
                query = match value {
                    SqlValue::String(value) => query.bind(value),
                    SqlValue::Int(value) => query.bind(*value),
                    SqlValue::Float(value) => query.bind(*value),
                    SqlValue::Bool(value) => query.bind(*value),
                    SqlValue::Null => query.bind(Option::<String>::None),
                };
            }
            query.execute(pool).await
        }

        async fn fetch_rows(
            pool: &DbPool,
            sql: &str,
            values: &[SqlValue],
        ) -> Result<Vec<DbRow>, sqlx::Error> {
            #[cfg(feature = "sqlite")]
            {
                let mut query = sqlx::query(sql);
                for value in values {
                    query = match value {
                        SqlValue::String(value) => query.bind(value),
                        SqlValue::Int(value) => query.bind(*value),
                        SqlValue::Float(value) => query.bind(*value),
                        SqlValue::Bool(value) => query.bind(*value),
                        SqlValue::Null => query.bind(Option::<String>::None),
                    };
                }
                query.fetch_all(pool).await
            }

            #[cfg(feature = "postgres")]
            {
                let sql = normalize_sql(sql, 1);
                let mut query = sqlx::query(&sql);
                for value in values {
                    query = match value {
                        SqlValue::String(value) => query.bind(value),
                        SqlValue::Int(value) => query.bind(*value),
                        SqlValue::Float(value) => query.bind(*value),
                        SqlValue::Bool(value) => query.bind(*value),
                        SqlValue::Null => query.bind(Option::<String>::None),
                    };
                }
                query.fetch_all(pool).await
            }
        }

        pub struct EntityQuery<T> {
            pub where_clauses: Vec<String>,
            pub values: Vec<SqlValue>,
            pub order_clauses: Vec<String>,
            pub page: Option<PageInput>,
            _marker: PhantomData<T>,
        }

        impl<T> Clone for EntityQuery<T> {
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

        impl<T> EntityQuery<T>
        where
            T: DatabaseEntity + FromSqlRow + Clone + Send + Sync,
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
                if let Some(order_sql) = order.to_sql_order() {
                    self.order_clauses.push(order_sql);
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

            fn build_select_sql(&self) -> String {
                let mut sql = format!(
                    "SELECT {} FROM {}",
                    T::column_names().join(", "),
                    T::TABLE_NAME
                );
                if !self.where_clauses.is_empty() {
                    sql.push_str(" WHERE ");
                    sql.push_str(&self.where_clauses.join(" AND "));
                }
                if !self.order_clauses.is_empty() {
                    sql.push_str(" ORDER BY ");
                    sql.push_str(&self.order_clauses.join(", "));
                }
                if let Some(page) = &self.page {
                    if let Some(limit) = page.limit() {
                        sql.push_str(&format!(" LIMIT {}", limit));
                    }
                    if page.offset() > 0 {
                        sql.push_str(&format!(" OFFSET {}", page.offset()));
                    }
                }
                sql
            }

            pub async fn fetch_all<P>(&self, provider: &P) -> Result<Vec<T>, sqlx::Error>
            where
                P: PoolProvider + ?Sized,
            {
                let sql = self.build_select_sql();
                let rows = fetch_rows(provider.pool(), &sql, &self.values).await?;
                rows.iter().map(T::from_row).collect()
            }

            pub async fn fetch_one<P>(&self, provider: &P) -> Result<Option<T>, sqlx::Error>
            where
                P: PoolProvider + ?Sized,
            {
                Ok(self.fetch_all(provider).await?.into_iter().next())
            }

            pub async fn count<P>(&self, provider: &P) -> Result<i64, sqlx::Error>
            where
                P: PoolProvider + ?Sized,
            {
                let mut sql = format!("SELECT COUNT(*) AS count FROM {}", T::TABLE_NAME);
                if !self.where_clauses.is_empty() {
                    sql.push_str(" WHERE ");
                    sql.push_str(&self.where_clauses.join(" AND "));
                }
                let rows = fetch_rows(provider.pool(), &sql, &self.values).await?;
                let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
                row.try_get::<i64, _>("count")
            }

            pub fn build_delete_sql(&self) -> (String, Vec<SqlValue>) {
                let mut sql = format!("DELETE FROM {}", T::TABLE_NAME);
                if !self.where_clauses.is_empty() {
                    sql.push_str(" WHERE ");
                    sql.push_str(&self.where_clauses.join(" AND "));
                }
                (sql, self.values.clone())
            }

            pub async fn fetch_connection<P>(
                &self,
                provider: &P,
            ) -> Result<Connection<T>, sqlx::Error>
            where
                P: PoolProvider + ?Sized,
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
                        has_next_page: false,
                        has_previous_page: offset > 0,
                        start_cursor: edges.first().map(|edge| edge.cursor.clone()),
                        end_cursor: edges.last().map(|edge| edge.cursor.clone()),
                        total_count: Some(total),
                    },
                    edges,
                })
            }
        }

        pub struct FindQuery<'a, T, W, O>
        where
            T: DatabaseEntity + FromSqlRow + Clone + Send + Sync,
        {
            pool: &'a DbPool,
            query: EntityQuery<T>,
            _marker: PhantomData<(W, O)>,
        }

        impl<'a, T, W, O> FindQuery<'a, T, W, O>
        where
            T: DatabaseEntity + FromSqlRow + Clone + Send + Sync,
        {
            pub fn new(pool: &'a DbPool) -> Self {
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

            pub async fn fetch_all(self) -> Result<Vec<T>, sqlx::Error> {
                self.query.fetch_all(self.pool).await
            }
        }

        pub struct CountQuery<'a, W> {
            pool: &'a DbPool,
            table: &'static str,
            filters: Vec<String>,
            values: Vec<SqlValue>,
            _marker: PhantomData<W>,
        }

        impl<'a, W> CountQuery<'a, W>
        where
            W: DatabaseFilter,
        {
            pub fn new(pool: &'a DbPool, table: &'static str) -> Self {
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
                let mut sql = format!("SELECT COUNT(*) AS count FROM {}", self.table);
                if !self.filters.is_empty() {
                    sql.push_str(" WHERE ");
                    sql.push_str(&self.filters.join(" AND "));
                }
                let rows = fetch_rows(self.pool, &sql, &self.values).await?;
                let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
                row.try_get::<i64, _>("count")
            }
        }
    }

    pub mod loaders {
        use super::{DbRow, HashMap, PhantomData};
        use async_graphql::dataloader::Loader;

        pub trait BatchLoadEntity:
            crate::graphql::orm::DatabaseEntity
            + crate::graphql::orm::FromSqlRow
            + Clone
            + Send
            + Sync
            + 'static
        {
            fn batch_column() -> &'static str;
            fn batch_key_from_row(row: &DbRow) -> Result<String, sqlx::Error>;
        }

        pub struct RelationLoader<T> {
            db: crate::db::Database,
            _marker: PhantomData<T>,
        }

        impl<T> RelationLoader<T> {
            pub fn new(db: crate::db::Database) -> Self {
                Self {
                    db,
                    _marker: PhantomData,
                }
            }
        }

        impl<T> Loader<String> for RelationLoader<T>
        where
            T: BatchLoadEntity,
        {
            type Value = Vec<T>;
            type Error = String;

            fn load(
                &self,
                keys: &[String],
            ) -> impl std::future::Future<
                Output = Result<HashMap<String, Self::Value>, Self::Error>,
            > + Send {
                let keys = keys.to_vec();
                let db = self.db.clone();
                async move {
                    if keys.is_empty() {
                        return Ok(HashMap::new());
                    }

                    let sql = if cfg!(feature = "postgres") {
                        let params = (1..=keys.len())
                            .map(|index| format!("${index}"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!(
                            "SELECT {} FROM {} WHERE {} IN ({})",
                            T::column_names().join(", "),
                            T::TABLE_NAME,
                            T::batch_column(),
                            params
                        )
                    } else {
                        let params = (0..keys.len()).map(|_| "?").collect::<Vec<_>>().join(", ");
                        format!(
                            "SELECT {} FROM {} WHERE {} IN ({})",
                            T::column_names().join(", "),
                            T::TABLE_NAME,
                            T::batch_column(),
                            params
                        )
                    };

                    let mut query = sqlx::query(&sql);
                    for key in &keys {
                        query = query.bind(key);
                    }

                    let rows = query
                        .fetch_all(db.pool())
                        .await
                        .map_err(|error| error.to_string())?;

                    let mut grouped: HashMap<String, Vec<T>> =
                        keys.into_iter().map(|key| (key, Vec::new())).collect();

                    for row in rows {
                        let key = T::batch_key_from_row(&row).map_err(|error| error.to_string())?;
                        let entity = T::from_row(&row).map_err(|error| error.to_string())?;
                        grouped.entry(key).or_default().push(entity);
                    }

                    Ok(grouped)
                }
            }
        }
    }
}

pub mod types {
    pub trait Record {}
}

pub mod prelude {
    pub use crate::db::Database;
    pub use crate::graphql::auth::AuthExt;
    pub use crate::graphql::filters::*;
    pub use crate::graphql::loaders::{BatchLoadEntity, RelationLoader};
    pub use crate::graphql::orm::*;
    pub use crate::graphql::pagination::*;
    pub use crate::{GraphQLEntity, GraphQLOperations, GraphQLRelations, mutation_result, schema_roots};
}
