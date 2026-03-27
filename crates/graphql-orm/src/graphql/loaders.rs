use super::{DbRow, HashMap, PhantomData};
use async_graphql::dataloader::Loader;
use std::hash::{Hash, Hasher};

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

#[derive(Clone, Debug)]
pub struct RelationQueryKey {
    pub relation: &'static str,
    pub parent_key: String,
    pub parent_value: crate::graphql::orm::SqlValue,
    pub fk_column: &'static str,
    pub where_signature: Option<String>,
    pub order_signature: Option<String>,
    pub page_signature: Option<String>,
    pub filter: Option<crate::graphql::orm::FilterExpression>,
    pub sorts: Vec<crate::graphql::orm::SortExpression>,
    pub pagination: Option<crate::graphql::orm::PaginationRequest>,
}

impl PartialEq for RelationQueryKey {
    fn eq(&self, other: &Self) -> bool {
        self.relation == other.relation
            && self.parent_key == other.parent_key
            && self.fk_column == other.fk_column
            && self.where_signature == other.where_signature
            && self.order_signature == other.order_signature
            && self.page_signature == other.page_signature
    }
}

impl Eq for RelationQueryKey {}

impl Hash for RelationQueryKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.relation.hash(state);
        self.parent_key.hash(state);
        self.fk_column.hash(state);
        self.where_signature.hash(state);
        self.order_signature.hash(state);
        self.page_signature.hash(state);
    }
}

#[derive(Clone, Debug)]
pub struct RelationLoadResult<T> {
    pub entities: Vec<T>,
    pub total_count: i64,
    pub has_next_page: bool,
    pub has_previous_page: bool,
    pub offset: i64,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct RelationGroupKey {
    relation: &'static str,
    fk_column: &'static str,
    where_signature: Option<String>,
    order_signature: Option<String>,
    page_signature: Option<String>,
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
    ) -> impl std::future::Future<Output = Result<HashMap<String, Self::Value>, Self::Error>> + Send
    {
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

impl<T> Loader<RelationQueryKey> for RelationLoader<T>
where
    T: BatchLoadEntity,
{
    type Value = RelationLoadResult<T>;
    type Error = String;

    fn load(
        &self,
        keys: &[RelationQueryKey],
    ) -> impl std::future::Future<
        Output = Result<HashMap<RelationQueryKey, Self::Value>, Self::Error>,
    > + Send {
        let keys = keys.to_vec();
        let db = self.db.clone();

        async move {
            use crate::graphql::orm::{
                DatabaseBackend, FilterExpression, PaginationRequest, SelectQuery, SortExpression,
                current_backend, fetch_rows, render_select_query,
            };
            use sqlx::Row;

            if keys.is_empty() {
                return Ok(HashMap::new());
            }

            let mut grouped_keys: HashMap<RelationGroupKey, Vec<RelationQueryKey>> = HashMap::new();
            for key in keys {
                let group_key = RelationGroupKey {
                    relation: key.relation,
                    fk_column: key.fk_column,
                    where_signature: key.where_signature.clone(),
                    order_signature: key.order_signature.clone(),
                    page_signature: key.page_signature.clone(),
                };
                grouped_keys.entry(group_key).or_default().push(key);
            }

            let mut results = HashMap::new();

            for group in grouped_keys.into_values() {
                let sample = group
                    .first()
                    .ok_or_else(|| "relation batch group should not be empty".to_string())?;

                let mut parent_values = Vec::with_capacity(group.len());
                let mut parent_key_order = Vec::with_capacity(group.len());
                for key in &group {
                    parent_values.push(key.parent_value.clone());
                    parent_key_order.push(key.parent_key.clone());
                }

                let placeholders = match current_backend() {
                    DatabaseBackend::Postgres => (1..=parent_values.len())
                        .map(|index| format!("${index}"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    _ => (0..parent_values.len())
                        .map(|_| "?".to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                };

                let parent_filter = FilterExpression::Raw {
                    clause: format!("{} IN ({})", sample.fk_column, placeholders),
                    values: parent_values,
                };
                let filter = match sample.filter.clone() {
                    Some(filter) => FilterExpression::And(vec![parent_filter, filter]),
                    None => parent_filter,
                };

                let mut sorts = vec![SortExpression {
                    clause: format!("{} ASC", sample.fk_column),
                }];
                sorts.extend(sample.sorts.clone());

                let rendered = render_select_query(
                    current_backend(),
                    &SelectQuery {
                        table: T::TABLE_NAME,
                        columns: T::column_names()
                            .iter()
                            .map(|column| (*column).to_string())
                            .chain(std::iter::once(format!(
                                "CAST({} AS TEXT) AS __gom_relation_key",
                                sample.fk_column
                            )))
                            .collect(),
                        filter: Some(filter),
                        sorts,
                        pagination: None,
                        count_only: false,
                    },
                );

                let rows = fetch_rows(db.pool(), &rendered.sql, &rendered.values)
                    .await
                    .map_err(|error| error.to_string())?;

                let mut grouped_entities: HashMap<String, Vec<T>> = parent_key_order
                    .iter()
                    .cloned()
                    .map(|parent_key| (parent_key, Vec::new()))
                    .collect();

                for row in rows {
                    let parent_key = row
                        .try_get::<String, _>("__gom_relation_key")
                        .map_err(|error| error.to_string())?;
                    let entity = T::from_row(&row).map_err(|error| error.to_string())?;
                    grouped_entities.entry(parent_key).or_default().push(entity);
                }

                for key in group {
                    let all_entities = grouped_entities.remove(&key.parent_key).unwrap_or_default();
                    let total_count = all_entities.len() as i64;
                    let PaginationRequest { limit, offset } =
                        key.pagination.clone().unwrap_or(PaginationRequest {
                            limit: None,
                            offset: 0,
                        });
                    let start = offset.max(0) as usize;
                    let end = match limit {
                        Some(limit) if limit >= 0 => start.saturating_add(limit as usize),
                        _ => all_entities.len(),
                    };
                    let has_previous_page = start > 0;
                    let has_next_page = end < all_entities.len();
                    let entities = all_entities
                        .into_iter()
                        .skip(start)
                        .take(end.saturating_sub(start))
                        .collect();

                    results.insert(
                        key,
                        RelationLoadResult {
                            entities,
                            total_count,
                            has_next_page,
                            has_previous_page,
                            offset,
                        },
                    );
                }
            }

            Ok(results)
        }
    }
}
