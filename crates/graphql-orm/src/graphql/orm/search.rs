use super::core::{SearchIndexDef, SearchWeight, SqlValue};
use super::dialect::{DatabaseBackend, SqlDialect};
use super::query::{
    DatabaseEntity, DatabaseFilter, DatabaseOrderBy, DatabaseSearchSchema, EntityQuery, FromSqlRow,
    PageInput, PoolProvider,
};
use super::{DbAuthContext, OrmBackend, WriteBackend};
use crate::graphql::filters::{SearchInput, SearchMode};
use crate::graphql::pagination::{PageInfo, encode_cursor};
use std::marker::PhantomData;

/// One weighted text fragment inside a generated search document.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchDocumentChunk {
    pub source: SearchDocumentSource,
    pub weight: SearchWeight,
    pub text: String,
}

/// Origin of a generated search document chunk.
#[derive(Clone, Debug, PartialEq)]
pub enum SearchDocumentSource {
    /// Text came from a local entity field.
    Field { field_name: &'static str },
    /// Text came from a configured relation field.
    RelationField {
        relation_field: &'static str,
        target_field: &'static str,
    },
}

/// Denormalized search document for one entity row.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchDocument {
    pub entity_pk: String,
    pub entity_pk_json: serde_json::Value,
    pub chunks: Vec<SearchDocumentChunk>,
}

impl SearchDocument {
    /// Concatenate all non-empty chunks into one document string.
    pub fn document_text(&self) -> String {
        self.chunks
            .iter()
            .map(|chunk| chunk.text.as_str())
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Concatenate non-empty chunks that use the requested search weight.
    pub fn text_for_weight(&self, weight: SearchWeight) -> String {
        self.chunks
            .iter()
            .filter(|chunk| chunk.weight == weight)
            .map(|chunk| chunk.text.as_str())
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// One scored search result returned by generated Rust search helpers.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit<T> {
    pub score: f64,
    pub entity: T,
}

/// GraphQL connection edge for generated search resolvers.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchConnectionEdge<T> {
    pub cursor: String,
    pub score: f64,
    pub node: T,
}

/// GraphQL-style connection returned by generated search resolvers.
#[derive(Clone, Debug)]
pub struct SearchConnection<T> {
    pub edges: Vec<SearchConnectionEdge<T>>,
    pub page_info: PageInfo,
}

/// Options for explicit search document rebuilds.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchRebuildOptions {
    /// Number of entity rows processed per rebuild batch.
    pub batch_size: usize,
    /// Whether rebuilds should delete search rows without matching entity rows.
    pub delete_orphans: bool,
}

impl Default for SearchRebuildOptions {
    fn default() -> Self {
        Self {
            batch_size: 500,
            delete_orphans: true,
        }
    }
}

/// Trait implemented by generated entities that have full-text search metadata.
pub trait SearchableEntity: DatabaseSearchSchema {
    /// Stable text key used in denormalized search tables.
    fn search_key(&self) -> String;
    /// JSON representation of the entity key for rebuild and diagnostics.
    fn search_key_json(&self) -> serde_json::Value;
    /// Build the current denormalized search document for this entity value.
    fn search_document(&self) -> SearchDocument;
}

/// Return the managed PostgreSQL/fallback search table name for a base table.
pub fn search_table_name(table_name: &str) -> String {
    format!("__graphql_orm_search_{}", sanitize_search_name(table_name))
}

/// Return the managed SQLite FTS5 table name for a base table.
pub fn sqlite_fts_table_name(table_name: &str) -> String {
    format!("__graphql_orm_fts_{}", sanitize_search_name(table_name))
}

/// Return the managed fallback token table name for a base table.
pub fn search_token_table_name(table_name: &str) -> String {
    format!(
        "__graphql_orm_search_token_{}",
        sanitize_search_name(table_name)
    )
}

/// Name of the shared search metadata table.
pub fn search_metadata_table_name() -> &'static str {
    "__graphql_orm_search_metadata"
}

/// Sanitize a table name for use in managed search helper table names.
pub fn sanitize_search_name(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// Normalize text for deterministic fallback search matching.
pub fn normalize_search_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut last_was_space = true;
    for ch in value.chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() {
            out.push(ch);
            last_was_space = false;
        } else if !last_was_space {
            out.push(' ');
            last_was_space = true;
        }
    }
    out.trim().to_string()
}

/// Tokenize text using the fallback tokenizer and minimum token length.
pub fn tokenize_search_text(value: &str, min_token_len: usize) -> Vec<String> {
    normalize_search_text(value)
        .split_whitespace()
        .filter(|token| token.chars().count() >= min_token_len)
        .map(str::to_string)
        .collect()
}

fn query_tokens(input: &SearchInput, min_token_len: usize) -> Vec<String> {
    tokenize_search_text(&input.query, min_token_len)
}

/// Compute a deterministic fallback score for a query against one search document.
pub fn fallback_score_document(
    input: &SearchInput,
    document: &SearchDocument,
    min_token_len: usize,
) -> f64 {
    let mode = input.mode.unwrap_or_default();
    let query = normalize_search_text(&input.query);
    if query.is_empty() {
        return 0.0;
    }

    match mode {
        SearchMode::Phrase => {
            let mut score = 0.0;
            for chunk in &document.chunks {
                if normalize_search_text(&chunk.text).contains(&query) {
                    score += chunk.weight.score_multiplier();
                }
            }
            score
        }
        SearchMode::Prefix => {
            let prefixes = query_tokens(input, min_token_len);
            if prefixes.is_empty() {
                return 0.0;
            }
            let mut score = 0.0;
            for chunk in &document.chunks {
                let tokens = tokenize_search_text(&chunk.text, min_token_len);
                for prefix in &prefixes {
                    let matches = tokens
                        .iter()
                        .filter(|token| token.starts_with(prefix))
                        .count();
                    score += matches as f64 * chunk.weight.score_multiplier();
                }
            }
            score
        }
        SearchMode::Plain | SearchMode::Web => {
            let tokens = query_tokens(input, min_token_len);
            if tokens.is_empty() {
                return 0.0;
            }
            let mut score = 0.0;
            for chunk in &document.chunks {
                let chunk_tokens = tokenize_search_text(&chunk.text, min_token_len);
                for token in &tokens {
                    let frequency = chunk_tokens
                        .iter()
                        .filter(|candidate| *candidate == token)
                        .count();
                    score += frequency as f64 * chunk.weight.score_multiplier();
                }
            }
            score
        }
    }
}

/// Convert a search input into a SQLite FTS5 query string.
pub fn sqlite_fts_query(input: &SearchInput, min_token_len: usize) -> String {
    match input.mode.unwrap_or_default() {
        SearchMode::Phrase => format!("\"{}\"", input.query.replace('"', "\"\"")),
        SearchMode::Prefix => tokenize_search_text(&input.query, min_token_len)
            .into_iter()
            .map(|token| format!("{token}*"))
            .collect::<Vec<_>>()
            .join(" "),
        SearchMode::Plain | SearchMode::Web => {
            tokenize_search_text(&input.query, min_token_len).join(" ")
        }
    }
}

/// Return the PostgreSQL tsquery function used for a search mode.
pub fn postgres_tsquery_function(mode: SearchMode) -> &'static str {
    match mode {
        SearchMode::Plain => "plainto_tsquery",
        SearchMode::Phrase => "phraseto_tsquery",
        SearchMode::Web => "websearch_to_tsquery",
        SearchMode::Prefix => "to_tsquery",
    }
}

/// Convert a prefix-mode search input into a PostgreSQL `to_tsquery` string.
pub fn postgres_prefix_query(input: &SearchInput, min_token_len: usize) -> String {
    tokenize_search_text(&input.query, min_token_len)
        .into_iter()
        .map(|token| format!("{token}:*"))
        .collect::<Vec<_>>()
        .join(" & ")
}

fn count_placeholders(clause: &str) -> usize {
    let chars = clause.chars().collect::<Vec<_>>();
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

/// Builder returned by generated `Entity::search(...)` helpers.
pub struct EntitySearchQuery<'a, T, W, B: OrmBackend> {
    pool: &'a B::Pool,
    search: SearchInput,
    query: EntityQuery<T, B>,
    _where: PhantomData<W>,
}

impl<'a, T, W, B> EntitySearchQuery<'a, T, W, B>
where
    B: OrmBackend,
    T: DatabaseEntity + SearchableEntity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    /// Create a search query builder for an entity, pool, and search input.
    pub fn new(pool: &'a B::Pool, search: SearchInput) -> Self {
        Self {
            pool,
            search,
            query: EntityQuery::new(),
            _where: PhantomData,
        }
    }

    /// Add a generated entity `where` filter to the search.
    pub fn filter(mut self, filter: W) -> Self
    where
        W: DatabaseFilter + Clone + Send + Sync + 'static,
    {
        self.query = self.query.filter_with_entity_matching(&filter);
        self
    }

    /// Add a generated order-by expression.
    pub fn order_by<O>(mut self, order: O) -> Self
    where
        O: DatabaseOrderBy,
    {
        self.query = self.query.order_by(&order);
        self
    }

    /// Apply the entity default sort after relevance where supported.
    pub fn default_order(mut self) -> Self {
        self.query = self.query.default_order();
        self
    }

    /// Limit the number of hits returned.
    pub fn limit(mut self, limit: i64) -> Self {
        let mut page = self.query.page.unwrap_or_default();
        page.limit = Some(limit);
        self.query.page = Some(page);
        self
    }

    /// Offset the returned hits.
    pub fn offset(mut self, offset: i64) -> Self {
        let mut page = self.query.page.unwrap_or_default();
        page.offset = Some(offset);
        self.query.page = Some(page);
        self
    }

    /// Apply a generated page input.
    pub fn paginate(mut self, page: PageInput) -> Self {
        self.query = self.query.paginate(&page);
        self
    }

    fn score_entities(&self, entities: Vec<T>) -> Vec<SearchHit<T>> {
        let min_token_len = T::search_index()
            .map(|index| index.min_token_len)
            .unwrap_or(2);
        let min_score = self.search.min_score.unwrap_or(0.0);
        let mut hits = entities
            .into_iter()
            .filter_map(|entity| {
                let score =
                    fallback_score_document(&self.search, &entity.search_document(), min_token_len);
                if score >= min_score && score > 0.0 {
                    Some(SearchHit { score, entity })
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        hits.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits
    }

    async fn try_fetch_native(&self) -> Option<Result<Vec<SearchHit<T>>, sqlx::Error>> {
        let index = T::search_index()?;
        if !index.enabled || self.query.requires_in_memory_filtering() {
            return None;
        }

        let rendered = match B::DIALECT {
            DatabaseBackend::Postgres => self.render_postgres_search(index),
            DatabaseBackend::Sqlite => self.render_sqlite_fts_search(index),
            DatabaseBackend::Mysql | DatabaseBackend::Mssql => return None,
        };

        let rows = match B::fetch_rows(self.pool, &rendered.0, &rendered.1).await {
            Ok(rows) => rows,
            Err(error) if index.fallback_enabled => {
                let _ = error;
                return None;
            }
            Err(error) => return Some(Err(error)),
        };

        Some(
            rows.iter()
                .map(|row| {
                    let entity = T::from_row(row)?;
                    let score = B::try_get_f64(row, "__gom_search_score")?;
                    Ok(SearchHit { score, entity })
                })
                .collect(),
        )
    }

    fn render_postgres_search(&self, index: &SearchIndexDef) -> (String, Vec<SqlValue>) {
        let mode = self.search.mode.unwrap_or_default();
        let query_text = if mode == SearchMode::Prefix {
            postgres_prefix_query(&self.search, index.min_token_len)
        } else {
            self.search.query.clone()
        };
        let function = postgres_tsquery_function(mode);
        let search_table = search_table_name(index.table_name);
        let mut sql = format!(
            "SELECT {}, ts_rank_cd(s.document_vector, q.query) AS __gom_search_score \
             FROM {} \
             JOIN {} s ON s.entity_pk = {}::text \
             CROSS JOIN {}('{}', ?) q(query) \
             WHERE s.document_vector @@ q.query",
            T::column_names().join(", "),
            T::TABLE_NAME,
            search_table,
            T::PRIMARY_KEY,
            function,
            index.language.replace('\'', "''")
        );
        let mut values = vec![SqlValue::String(query_text)];
        self.append_filter_sql(&mut sql, &mut values);
        sql.push_str(" ORDER BY __gom_search_score DESC");
        if !T::DEFAULT_SORT.trim().is_empty() {
            sql.push_str(", ");
            sql.push_str(T::DEFAULT_SORT);
        }
        (sql, values)
    }

    fn render_sqlite_fts_search(&self, index: &SearchIndexDef) -> (String, Vec<SqlValue>) {
        let fts_table = sqlite_fts_table_name(index.table_name);
        let mut sql = format!(
            "SELECT {}, bm25({}, 1.0, 0.7, 0.4, 0.1, 0.1) * -1.0 AS __gom_search_score \
             FROM {} \
             JOIN {} ON {}.entity_pk = CAST({} AS TEXT) \
             WHERE {} MATCH ?",
            T::column_names().join(", "),
            fts_table,
            T::TABLE_NAME,
            fts_table,
            fts_table,
            T::PRIMARY_KEY,
            fts_table,
        );
        let mut values = vec![SqlValue::String(sqlite_fts_query(
            &self.search,
            index.min_token_len,
        ))];
        self.append_filter_sql(&mut sql, &mut values);
        sql.push_str(" ORDER BY __gom_search_score DESC");
        if !T::DEFAULT_SORT.trim().is_empty() {
            sql.push_str(", ");
            sql.push_str(T::DEFAULT_SORT);
        }
        (sql, values)
    }

    fn append_filter_sql(&self, sql: &mut String, values: &mut Vec<SqlValue>) {
        let mut value_index = 0usize;
        for clause in &self.query.where_clauses {
            let placeholder_count = count_placeholders(clause);
            let start_index = values.len() + 1;
            let rendered = B::DIALECT.normalize_sql(clause, start_index);
            sql.push_str(" AND (");
            sql.push_str(&rendered);
            sql.push(')');
            values.extend(
                self.query.values[value_index..value_index + placeholder_count]
                    .iter()
                    .cloned(),
            );
            value_index += placeholder_count;
        }
    }

    fn paginate_hits(&self, hits: Vec<SearchHit<T>>) -> SearchConnection<T> {
        let total = hits.len() as i64;
        let offset = self
            .query
            .page
            .as_ref()
            .map(|page| page.offset())
            .unwrap_or(0) as usize;
        let limit = self
            .query
            .page
            .as_ref()
            .and_then(|page| page.limit())
            .map(|limit| limit.max(0) as usize);
        let page_hits = if offset >= hits.len() {
            Vec::new()
        } else {
            let iter = hits.into_iter().skip(offset);
            match limit {
                Some(limit) => iter.take(limit).collect(),
                None => iter.collect(),
            }
        };
        let edges = page_hits
            .into_iter()
            .enumerate()
            .map(|(index, hit)| SearchConnectionEdge {
                cursor: encode_cursor((offset + index) as i64),
                score: hit.score,
                node: hit.entity,
            })
            .collect::<Vec<_>>();
        SearchConnection {
            page_info: PageInfo {
                has_next_page: (offset as i64 + edges.len() as i64) < total,
                has_previous_page: offset > 0,
                start_cursor: edges.first().map(|edge| edge.cursor.clone()),
                end_cursor: edges.last().map(|edge| edge.cursor.clone()),
                total_count: Some(total),
            },
            edges,
        }
    }

    /// Execute the search and return all matching hits.
    pub async fn fetch_all(self) -> Result<Vec<SearchHit<T>>, sqlx::Error> {
        if let Some(result) = self.try_fetch_native().await {
            return result;
        }
        let mut query = self.query.clone();
        query.page = None;
        let entities = query.fetch_all(&PoolRef { pool: self.pool }).await?;
        Ok(self.score_entities(entities))
    }

    /// Execute the search with an optional database auth context.
    pub async fn fetch_all_with_auth(
        self,
        auth: Option<&DbAuthContext>,
    ) -> Result<Vec<SearchHit<T>>, sqlx::Error> {
        if auth.is_none() {
            if let Some(result) = self.try_fetch_native().await {
                return result;
            }
        }
        let mut query = self.query.clone();
        query.page = None;
        let entities = query
            .fetch_all_with_auth(&PoolRef { pool: self.pool }, auth)
            .await?;
        Ok(self.score_entities(entities))
    }

    /// Execute the search and return a GraphQL-style connection.
    pub async fn fetch_connection(self) -> Result<SearchConnection<T>, sqlx::Error> {
        let hits = self.clone_for_fetch().fetch_all().await?;
        Ok(self.paginate_hits(hits))
    }

    /// Execute the search with auth and return a GraphQL-style connection.
    pub async fn fetch_connection_with_auth(
        self,
        auth: Option<&DbAuthContext>,
    ) -> Result<SearchConnection<T>, sqlx::Error> {
        let hits = self.clone_for_fetch().fetch_all_with_auth(auth).await?;
        Ok(self.paginate_hits(hits))
    }

    fn clone_for_fetch(&self) -> Self {
        Self {
            pool: self.pool,
            search: self.search.clone(),
            query: self.query.clone(),
            _where: PhantomData,
        }
    }
}

struct PoolRef<'a, B: OrmBackend> {
    pool: &'a B::Pool,
}

impl<B: OrmBackend> PoolProvider<B> for PoolRef<'_, B> {
    fn pool(&self) -> &B::Pool {
        self.pool
    }
}

/// Upsert a generated search document inside an existing write transaction.
pub async fn upsert_search_document_on<B>(
    executor: &mut <B::Database as sqlx::Database>::Connection,
    index: &SearchIndexDef,
    document: &SearchDocument,
) -> Result<(), sqlx::Error>
where
    B: WriteBackend,
    for<'c> &'c mut <B::Database as sqlx::Database>::Connection:
        sqlx::Executor<'c, Database = B::Database> + Send,
{
    match B::DIALECT {
        DatabaseBackend::Postgres => {
            upsert_postgres_search_document_on::<B>(executor, index, document).await
        }
        DatabaseBackend::Sqlite => {
            upsert_sqlite_search_document_on::<B>(executor, index, document).await
        }
        DatabaseBackend::Mysql | DatabaseBackend::Mssql => Err(sqlx::Error::Protocol(format!(
            "full-text search execution is not implemented for {}",
            B::DIALECT.name()
        ))),
    }
}

async fn upsert_postgres_search_document_on<B>(
    executor: &mut <B::Database as sqlx::Database>::Connection,
    index: &SearchIndexDef,
    document: &SearchDocument,
) -> Result<(), sqlx::Error>
where
    B: WriteBackend,
    for<'c> &'c mut <B::Database as sqlx::Database>::Connection:
        sqlx::Executor<'c, Database = B::Database> + Send,
{
    let table = search_table_name(index.table_name);
    let sql = format!(
        "INSERT INTO {table} \
         (entity_pk, entity_pk_json, document_text, document_vector, updated_at) \
         VALUES (?, ?::jsonb, ?, \
         setweight(to_tsvector('{}', ?), 'A') || \
         setweight(to_tsvector('{}', ?), 'B') || \
         setweight(to_tsvector('{}', ?), 'C') || \
         setweight(to_tsvector('{}', ?), 'D'), ?) \
         ON CONFLICT (entity_pk) DO UPDATE SET \
         entity_pk_json = EXCLUDED.entity_pk_json, \
         document_text = EXCLUDED.document_text, \
         document_vector = EXCLUDED.document_vector, \
         updated_at = EXCLUDED.updated_at",
        index.language, index.language, index.language, index.language
    );
    let values = vec![
        SqlValue::String(document.entity_pk.clone()),
        SqlValue::Json(document.entity_pk_json.clone()),
        SqlValue::String(document.document_text()),
        SqlValue::String(document.text_for_weight(SearchWeight::A)),
        SqlValue::String(document.text_for_weight(SearchWeight::B)),
        SqlValue::String(document.text_for_weight(SearchWeight::C)),
        SqlValue::String(document.text_for_weight(SearchWeight::D)),
        SqlValue::Int(current_epoch_seconds()),
    ];
    super::execute_with_binds_on::<B, _>(&mut *executor, &sql, &values).await?;
    Ok(())
}

async fn upsert_sqlite_search_document_on<B>(
    executor: &mut <B::Database as sqlx::Database>::Connection,
    index: &SearchIndexDef,
    document: &SearchDocument,
) -> Result<(), sqlx::Error>
where
    B: WriteBackend,
    for<'c> &'c mut <B::Database as sqlx::Database>::Connection:
        sqlx::Executor<'c, Database = B::Database> + Send,
{
    let table = sqlite_fts_table_name(index.table_name);
    let delete_sql = format!("DELETE FROM {table} WHERE entity_pk = ?");
    super::execute_with_binds_on::<B, _>(
        &mut *executor,
        &delete_sql,
        &[SqlValue::String(document.entity_pk.clone())],
    )
    .await?;
    let insert_sql = format!(
        "INSERT INTO {table} \
         (entity_pk, weight_a, weight_b, weight_c, weight_d, document_text) \
         VALUES (?, ?, ?, ?, ?, ?)"
    );
    let values = vec![
        SqlValue::String(document.entity_pk.clone()),
        SqlValue::String(document.text_for_weight(SearchWeight::A)),
        SqlValue::String(document.text_for_weight(SearchWeight::B)),
        SqlValue::String(document.text_for_weight(SearchWeight::C)),
        SqlValue::String(document.text_for_weight(SearchWeight::D)),
        SqlValue::String(document.document_text()),
    ];
    super::execute_with_binds_on::<B, _>(&mut *executor, &insert_sql, &values).await?;
    Ok(())
}

/// Delete a generated search document inside an existing write transaction.
pub async fn delete_search_document_on<B>(
    executor: &mut <B::Database as sqlx::Database>::Connection,
    index: &SearchIndexDef,
    entity_pk: &str,
) -> Result<(), sqlx::Error>
where
    B: WriteBackend,
    for<'c> &'c mut <B::Database as sqlx::Database>::Connection:
        sqlx::Executor<'c, Database = B::Database> + Send,
{
    let table = match B::DIALECT {
        DatabaseBackend::Postgres => search_table_name(index.table_name),
        DatabaseBackend::Sqlite => sqlite_fts_table_name(index.table_name),
        DatabaseBackend::Mysql | DatabaseBackend::Mssql => {
            return Err(sqlx::Error::Protocol(format!(
                "full-text search execution is not implemented for {}",
                B::DIALECT.name()
            )));
        }
    };
    let sql = format!("DELETE FROM {table} WHERE entity_pk = ?");
    super::execute_with_binds_on::<B, _>(
        &mut *executor,
        &sql,
        &[SqlValue::String(entity_pk.to_string())],
    )
    .await?;
    Ok(())
}

fn current_epoch_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}
