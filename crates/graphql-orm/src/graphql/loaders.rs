use crate::graphql::orm::{DatabaseBackend, DbAuthContext, DefaultBackend, OrmBackend};
use async_graphql::dataloader::Loader;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum RelationKeyPartKind {
    String,
    Uuid,
    Int,
    Float,
    Bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct RelationKey {
    parts: Vec<String>,
}

impl RelationKey {
    pub fn new(parts: Vec<String>) -> Self {
        Self { parts }
    }

    pub fn single(part: String) -> Self {
        Self { parts: vec![part] }
    }

    pub fn from_sql_values(values: &[crate::graphql::orm::SqlValue]) -> Self {
        Self {
            parts: values.iter().map(sql_value_key_part).collect(),
        }
    }

    pub fn parts(&self) -> &[String] {
        &self.parts
    }
}

pub trait BatchLoadEntity<B: OrmBackend = DefaultBackend>:
    crate::graphql::orm::DatabaseEntity
    + crate::graphql::orm::FromSqlRow<B>
    + Clone
    + Send
    + Sync
    + 'static
{
    fn batch_column() -> &'static str;
    fn batch_key_from_row(row: &B::Row) -> crate::Result<String>;

    fn batch_columns() -> Vec<&'static str> {
        vec![Self::batch_column()]
    }

    fn batch_relation_key_from_row(row: &B::Row) -> crate::Result<RelationKey> {
        Self::batch_key_from_row(row).map(RelationKey::single)
    }
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
    pub auth_context: Option<DbAuthContext>,
}

#[derive(Clone, Debug)]
pub struct CompositeRelationQueryKey {
    pub relation: &'static str,
    pub parent_key: RelationKey,
    pub parent_values: Vec<crate::graphql::orm::SqlValue>,
    pub fk_columns: Vec<&'static str>,
    pub key_part_kinds: Vec<RelationKeyPartKind>,
    pub where_signature: Option<String>,
    pub order_signature: Option<String>,
    pub page_signature: Option<String>,
    pub filter: Option<crate::graphql::orm::FilterExpression>,
    pub sorts: Vec<crate::graphql::orm::SortExpression>,
    pub pagination: Option<crate::graphql::orm::PaginationRequest>,
    pub auth_context: Option<DbAuthContext>,
}

impl PartialEq for RelationQueryKey {
    fn eq(&self, other: &Self) -> bool {
        self.relation == other.relation
            && self.parent_key == other.parent_key
            && self.fk_column == other.fk_column
            && self.where_signature == other.where_signature
            && self.order_signature == other.order_signature
            && self.page_signature == other.page_signature
            && self.auth_context.as_ref().map(DbAuthContext::canonical_key)
                == other
                    .auth_context
                    .as_ref()
                    .map(DbAuthContext::canonical_key)
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
        self.auth_context
            .as_ref()
            .map(DbAuthContext::canonical_key)
            .hash(state);
    }
}

impl PartialEq for CompositeRelationQueryKey {
    fn eq(&self, other: &Self) -> bool {
        self.relation == other.relation
            && self.parent_key == other.parent_key
            && self.fk_columns == other.fk_columns
            && self.where_signature == other.where_signature
            && self.order_signature == other.order_signature
            && self.page_signature == other.page_signature
            && self.auth_context.as_ref().map(DbAuthContext::canonical_key)
                == other
                    .auth_context
                    .as_ref()
                    .map(DbAuthContext::canonical_key)
    }
}

impl Eq for CompositeRelationQueryKey {}

impl Hash for CompositeRelationQueryKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.relation.hash(state);
        self.parent_key.hash(state);
        self.fk_columns.hash(state);
        self.where_signature.hash(state);
        self.order_signature.hash(state);
        self.page_signature.hash(state);
        self.auth_context
            .as_ref()
            .map(DbAuthContext::canonical_key)
            .hash(state);
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

/// Generated resolver support. Sanitize host policy failures at a relationship
/// boundary while retaining the public authorization-misconfiguration code.
#[doc(hidden)]
pub fn relation_authorization_error(error: async_graphql::Error) -> async_graphql::Error {
    use crate::graphql::errors::{OrmErrorCode, OrmPublicError};
    let code = if error
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.get("code"))
        == Some(&async_graphql::Value::String(
            "AUTHORIZATION_MISCONFIGURED".into(),
        )) {
        OrmErrorCode::AuthorizationMisconfigured
    } else {
        OrmErrorCode::Forbidden
    };
    OrmPublicError::new(code)
        .with_internal(format!("{error:?}"))
        .into_graphql_error()
}

/// Generated resolver support for current SQL visibility and batch partitioning.
#[doc(hidden)]
pub fn with_relation_visibility<T, B>(
    mut key: CompositeRelationQueryKey,
    visibility: &crate::graphql::orm::ReadVisibility,
) -> async_graphql::Result<CompositeRelationQueryKey>
where
    T: crate::graphql::orm::Entity + 'static,
    B: OrmBackend,
{
    visibility.validate::<T, B>()?;
    if let Some(predicate) = visibility.predicate_expression() {
        key.filter = Some(match key.filter {
            Some(filter) => {
                crate::graphql::orm::FilterExpression::And(vec![filter, predicate.clone()])
            }
            None => predicate.clone(),
        });
        key.where_signature = Some(format!("{:?}", key.filter));
    }
    Ok(key)
}

/// Generated resolver support. Check selected target fields on authoritative
/// rows before returning a relationship object. This also keeps a field denial
/// on a nullable target from exposing a partially resolved target object.
#[doc(hidden)]
pub async fn authorize_relation_fields<T, B>(
    db: &crate::db::Database<B>,
    ctx: &async_graphql::Context<'_>,
    rows: &[T],
    connection: bool,
) -> async_graphql::Result<()>
where
    T: crate::graphql::orm::Entity + Send + Sync + 'static,
    B: OrmBackend,
{
    // Use execution-pruned selections while retaining the real child context:
    // field arguments, aliases, and path-dependent host policies must agree
    // with ordinary target field resolution.
    let fields = relation_selected_fields(ctx, &ctx.item.node.selection_set.node);
    if connection {
        for edge in fields
            .iter()
            .filter(|field| field.node.name.node.as_str().eq_ignore_ascii_case("edges"))
        {
            let edge_ctx = ctx.with_field(edge);
            for node in relation_selected_fields(ctx, &edge.node.selection_set.node)
                .into_iter()
                .filter(|field| field.node.name.node.as_str().eq_ignore_ascii_case("node"))
            {
                let selected = relation_selected_fields(ctx, &node.node.selection_set.node);
                for (index, row) in rows.iter().enumerate() {
                    let indexed_ctx = edge_ctx.with_index(index);
                    let node_ctx = indexed_ctx.with_field(node);
                    check_relation_fields(db, &node_ctx, &selected, ::std::slice::from_ref(row))
                        .await?;
                }
            }
        }
    } else {
        check_relation_fields(db, ctx, &fields, rows).await?;
    }
    Ok(())
}

fn relation_selected_fields<'a>(
    ctx: &'a async_graphql::Context<'_>,
    selection: &'a async_graphql::parser::types::SelectionSet,
) -> Vec<&'a async_graphql::Positioned<async_graphql::parser::types::Field>> {
    use async_graphql::parser::types::Selection;
    let mut fields = Vec::new();
    for item in &selection.items {
        match &item.node {
            Selection::Field(field) => fields.push(field),
            Selection::FragmentSpread(spread) => {
                if let Some(fragment) = ctx.query_env.fragments.get(&spread.node.fragment_name.node)
                {
                    fields.extend(relation_selected_fields(
                        ctx,
                        &fragment.node.selection_set.node,
                    ));
                }
            }
            Selection::InlineFragment(fragment) => fields.extend(relation_selected_fields(
                ctx,
                &fragment.node.selection_set.node,
            )),
        }
    }
    fields
}

async fn check_relation_fields<T, B>(
    db: &crate::db::Database<B>,
    ctx: &async_graphql::Context<'_>,
    selected: &[&async_graphql::Positioned<async_graphql::parser::types::Field>],
    rows: &[T],
) -> async_graphql::Result<()>
where
    T: crate::graphql::orm::Entity + Send + Sync + 'static,
    B: OrmBackend,
{
    for row in rows {
        for selection in selected {
            let Some(field) = T::repository_field_policies().iter().find(|field| {
                field.api_name == selection.node.name.node.as_str() && field.read_policy.is_some()
            }) else {
                continue;
            };
            let field_ctx = ctx.with_field(selection);
            db.ensure_readable_field(
                &field_ctx,
                T::entity_name(),
                field.api_name,
                field.read_policy,
                Some(row),
            )
            .await
            .map_err(relation_authorization_error)?;
        }
    }
    Ok(())
}

/// Generated resolver support for callback-only/prefiltered target policies.
/// Count authorized rows before paging, retaining only the requested window.
/// Exhausting the host's scan budget fails closed rather than publishing a
/// partial count or treating denied rows as end-of-data.
#[doc(hidden)]
pub async fn scan_authorized_relation<T, B>(
    db: &crate::db::Database<B>,
    ctx: &async_graphql::Context<'_>,
    mut query: crate::graphql::orm::EntityQuery<T, B>,
    auth: Option<&DbAuthContext>,
    visibility: &crate::graphql::orm::ReadVisibility,
    page: crate::graphql::orm::PaginationRequest,
) -> async_graphql::Result<RelationLoadResult<T>>
where
    B: OrmBackend,
    T: BatchLoadEntity<B> + crate::graphql::orm::Entity,
{
    use crate::graphql::errors::{OrmErrorCode, OrmPublicError};
    use crate::graphql::orm::{EntityAccessSurface, PageInput, PaginationConfig, SqlDialect};
    let invalid =
        || OrmPublicError::new(OrmErrorCode::AuthorizationMisconfigured).into_graphql_error();
    let config = db.authorized_scan_config().ok_or_else(invalid)?;
    if config.batch_size == 0
        || config.max_scanned_rows == 0
        || query.requires_in_memory_filtering()
    {
        return Err(invalid());
    }
    query = query.with_read_visibility(visibility)?;
    // Stable source batches must not depend on non-unique caller ordering alone.
    for column in T::PRIMARY_KEYS {
        if !query
            .order_clauses
            .iter()
            .any(|clause| crate::graphql::orm::sort_clause_mentions_column(clause, column))
        {
            query
                .order_clauses
                .push(format!("{} ASC", B::DIALECT.quote_identifier(column)));
        }
    }
    let offset = page.offset.max(0);
    let limit = page.limit.ok_or_else(invalid)?.max(0);
    let mut entities = Vec::new();
    let mut total_count = 0_i64;
    let mut scanned = 0_u32;
    loop {
        let size = config.batch_size.min(config.max_scanned_rows - scanned);
        if size == 0 {
            return Err(invalid());
        }
        let rows = query
            .clone()
            .paginate(&PageInput {
                limit: Some(i64::from(size)),
                offset: Some(i64::from(scanned)),
            })
            .fetch_all_with_auth_and_pagination(db, auth, PaginationConfig::unbounded())
            .await
            .map_err(|error| OrmPublicError::from(error).into_graphql_error())?;
        let short = rows.len() < size as usize;
        scanned += rows.len() as u32;
        for row in rows {
            if db
                .can_read_row(
                    Some(ctx),
                    T::entity_name(),
                    T::metadata().read_policy,
                    EntityAccessSurface::GraphqlRelation,
                    &row,
                )
                .await
                .map_err(relation_authorization_error)?
            {
                if total_count >= offset && entities.len() < limit as usize {
                    entities.push(row);
                }
                total_count += 1;
            }
        }
        if short {
            break;
        }
    }
    Ok(RelationLoadResult {
        has_next_page: offset.saturating_add(entities.len() as i64) < total_count,
        has_previous_page: offset > 0,
        entities,
        total_count,
        offset,
    })
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct RelationGroupKey {
    relation: &'static str,
    fk_columns: Vec<&'static str>,
    key_part_kinds: Vec<RelationKeyPartKind>,
    where_signature: Option<String>,
    order_signature: Option<String>,
    page_signature: Option<String>,
    auth_context_key: Option<String>,
}

fn sql_value_key_part(value: &crate::graphql::orm::SqlValue) -> String {
    match value {
        crate::graphql::orm::SqlValue::String(value) => value.clone(),
        crate::graphql::orm::SqlValue::Uuid(value) => value.to_string(),
        crate::graphql::orm::SqlValue::Int(value) => value.to_string(),
        crate::graphql::orm::SqlValue::Float(value) => value.to_string(),
        crate::graphql::orm::SqlValue::Decimal(value) => value.value().to_string(),
        crate::graphql::orm::SqlValue::Bool(value) => value.to_string(),
        crate::graphql::orm::SqlValue::Bytes(value) => String::from_utf8_lossy(value).to_string(),
        crate::graphql::orm::SqlValue::Json(value) => value.to_string(),
        crate::graphql::orm::SqlValue::StringNull
        | crate::graphql::orm::SqlValue::BytesNull
        | crate::graphql::orm::SqlValue::JsonNull
        | crate::graphql::orm::SqlValue::UuidNull
        | crate::graphql::orm::SqlValue::IntNull
        | crate::graphql::orm::SqlValue::FloatNull
        | crate::graphql::orm::SqlValue::DecimalNull(_)
        | crate::graphql::orm::SqlValue::BoolNull
        | crate::graphql::orm::SqlValue::Null => String::new(),
    }
}

pub fn relation_key_projection(
    dialect: DatabaseBackend,
    column: &str,
    kind: RelationKeyPartKind,
) -> String {
    match kind {
        RelationKeyPartKind::Bool => match dialect {
            DatabaseBackend::Mssql => {
                format!("CASE WHEN {column} = 1 THEN 'true' ELSE 'false' END")
            }
            DatabaseBackend::Postgres => {
                format!("CASE WHEN {column} THEN 'true' ELSE 'false' END")
            }
            DatabaseBackend::Sqlite | DatabaseBackend::Mysql => {
                format!("CASE WHEN {column} THEN 'true' ELSE 'false' END")
            }
        },
        RelationKeyPartKind::Float => match dialect {
            DatabaseBackend::Sqlite => format!("printf('%.17g', {column})"),
            _ => crate::graphql::orm::SqlDialect::relation_key_cast(&dialect, column),
        },
        RelationKeyPartKind::String | RelationKeyPartKind::Uuid | RelationKeyPartKind::Int => {
            crate::graphql::orm::SqlDialect::relation_key_cast(&dialect, column)
        }
    }
}

#[doc(hidden)]
pub fn relation_key_filter(
    columns: &[&'static str],
    parent_values: &[Vec<crate::graphql::orm::SqlValue>],
) -> crate::graphql::orm::FilterExpression {
    use crate::graphql::orm::{FilterExpression, SqlValue};

    if columns.len() == 1 {
        let values = parent_values
            .iter()
            .filter_map(|parts| parts.first().cloned())
            .collect::<Vec<SqlValue>>();
        let placeholders = std::iter::repeat_n("?", values.len())
            .collect::<Vec<_>>()
            .join(", ");
        return FilterExpression::trusted_fragment(
            format!("{} IN ({})", columns[0], placeholders),
            values,
        );
    }

    let mut values = Vec::new();
    let clauses = parent_values
        .iter()
        .map(|parts| {
            values.extend(parts.iter().cloned());
            let predicates = columns
                .iter()
                .map(|column| format!("{column} = ?"))
                .collect::<Vec<_>>()
                .join(" AND ");
            format!("({predicates})")
        })
        .collect::<Vec<_>>()
        .join(" OR ");

    FilterExpression::trusted_fragment(clauses, values)
}

fn relation_key_alias(index: usize) -> String {
    format!("__gom_relation_key_{index}")
}

async fn load_composite_relation_keys<T, B>(
    db: crate::db::Database<B>,
    keys: Vec<CompositeRelationQueryKey>,
) -> Result<HashMap<CompositeRelationQueryKey, RelationLoadResult<T>>, String>
where
    B: OrmBackend,
    T: BatchLoadEntity<B>,
{
    use crate::graphql::orm::{
        FilterExpression, PaginationRequest, SelectQuery, SortExpression, SqlDialect,
        render_filter_expression, render_select_query,
    };

    if keys.is_empty() {
        return Ok(HashMap::new());
    }

    let mut grouped_keys: HashMap<RelationGroupKey, Vec<CompositeRelationQueryKey>> =
        HashMap::new();
    for key in keys {
        let group_key = RelationGroupKey {
            relation: key.relation,
            fk_columns: key.fk_columns.clone(),
            key_part_kinds: key.key_part_kinds.clone(),
            where_signature: key.where_signature.clone(),
            order_signature: key.order_signature.clone(),
            page_signature: key.page_signature.clone(),
            auth_context_key: key.auth_context.as_ref().map(DbAuthContext::canonical_key),
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
            parent_values.push(key.parent_values.clone());
            parent_key_order.push(key.parent_key.clone());
        }

        let parent_filter = relation_key_filter(&sample.fk_columns, &parent_values);
        let filter = match sample.filter.clone() {
            Some(filter) => FilterExpression::And(vec![parent_filter, filter]),
            None => parent_filter,
        };

        let mut sorts = sample
            .fk_columns
            .iter()
            .map(|column| SortExpression {
                clause: format!("{column} ASC"),
                values: Vec::new(),
            })
            .collect::<Vec<_>>();
        sorts.extend(sample.sorts.clone());
        for primary_key in T::PRIMARY_KEYS {
            if !sorts.iter().any(|sort| {
                crate::graphql::orm::sort_clause_mentions_column(&sort.clause, primary_key)
            }) {
                sorts.push(SortExpression::unbound(format!("{primary_key} ASC")));
            }
        }
        let sort_sql = sorts
            .iter()
            .map(|sort| sort.clause.clone())
            .collect::<Vec<_>>()
            .join(", ");

        let relation_key_columns = sample
            .fk_columns
            .iter()
            .zip(sample.key_part_kinds.iter())
            .enumerate()
            .map(|(index, (column, kind))| {
                format!(
                    "{} AS {}",
                    relation_key_projection(B::DIALECT, column, *kind),
                    relation_key_alias(index)
                )
            })
            .collect::<Vec<_>>();

        if let Some(PaginationRequest { limit, offset }) = sample.pagination.clone() {
            let partition_columns = sample.fk_columns.join(", ");
            let mut paged_columns = T::column_names()
                .iter()
                .map(|column| (*column).to_string())
                .chain(relation_key_columns.clone())
                .collect::<Vec<_>>();
            paged_columns.push(format!(
                "ROW_NUMBER() OVER (PARTITION BY {partition_columns} ORDER BY {sort_sql}) AS __gom_relation_row_number"
            ));
            paged_columns.push(format!(
                "COUNT(*) OVER (PARTITION BY {partition_columns}) AS __gom_relation_total_count"
            ));

            let rendered_inner = render_select_query(
                B::DIALECT,
                &SelectQuery {
                    table: T::TABLE_NAME,
                    columns: paged_columns,
                    filter: Some(filter.clone()),
                    sorts: Vec::new(),
                    pagination: None,
                    count_only: false,
                },
            );

            let start = offset.max(0);
            let mut row_values = rendered_inner.values.clone();
            let mut row_sql = format!(
                "SELECT * FROM ({}) __gom_relation_page WHERE __gom_relation_row_number > {}",
                rendered_inner.sql,
                B::DIALECT.placeholder(row_values.len() + 1)
            );
            row_values.push(crate::graphql::orm::SqlValue::Int(start));
            if let Some(limit) = limit {
                let end = start.saturating_add(limit.max(0));
                row_sql.push_str(&format!(
                    " AND __gom_relation_row_number <= {}",
                    B::DIALECT.placeholder(row_values.len() + 1)
                ));
                row_values.push(crate::graphql::orm::SqlValue::Int(end));
            }
            row_sql.push_str(" ORDER BY ");
            row_sql.push_str(&sort_sql);
            row_sql.push_str(", __gom_relation_row_number ASC");

            let mut count_values = Vec::new();
            let mut next_index = 1usize;
            let where_sql =
                render_filter_expression(B::DIALECT, &filter, &mut next_index, &mut count_values);
            let count_columns = relation_key_columns
                .iter()
                .cloned()
                .chain(std::iter::once(
                    "COUNT(*) AS __gom_relation_total_count".to_string(),
                ))
                .collect::<Vec<_>>();
            let mut count_sql =
                format!("SELECT {} FROM {}", count_columns.join(", "), T::TABLE_NAME);
            if !where_sql.is_empty() {
                count_sql.push_str(" WHERE ");
                count_sql.push_str(&where_sql);
            }
            count_sql.push_str(" GROUP BY ");
            count_sql.push_str(&partition_columns);

            let (count_rows, rows) = B::fetch_rows_pair_with_auth(
                db.pool(),
                &count_sql,
                &count_values,
                &row_sql,
                &row_values,
                sample.auth_context.as_ref(),
            )
            .await
            .map_err(|error| error.to_string())?;

            let mut total_counts: HashMap<RelationKey, i64> = parent_key_order
                .iter()
                .cloned()
                .map(|parent_key| (parent_key, 0))
                .collect();
            for row in count_rows {
                let mut key_parts = Vec::with_capacity(sample.fk_columns.len());
                for index in 0..sample.fk_columns.len() {
                    let alias = relation_key_alias(index);
                    let part =
                        B::try_get_string(&row, &alias).map_err(|error| error.to_string())?;
                    key_parts.push(part);
                }
                let parent_key = RelationKey::new(key_parts);
                let total = B::try_get_i64(&row, "__gom_relation_total_count")
                    .map_err(|error| error.to_string())?;
                total_counts.insert(parent_key, total);
            }

            let mut grouped_entities: HashMap<RelationKey, Vec<T>> = parent_key_order
                .iter()
                .cloned()
                .map(|parent_key| (parent_key, Vec::new()))
                .collect();
            for row in rows {
                let mut key_parts = Vec::with_capacity(sample.fk_columns.len());
                for index in 0..sample.fk_columns.len() {
                    let alias = relation_key_alias(index);
                    let part =
                        B::try_get_string(&row, &alias).map_err(|error| error.to_string())?;
                    key_parts.push(part);
                }
                let parent_key = RelationKey::new(key_parts);
                let entity = T::from_row(&row).map_err(|error| error.to_string())?;
                grouped_entities.entry(parent_key).or_default().push(entity);
            }

            for key in group {
                let entities = grouped_entities.remove(&key.parent_key).unwrap_or_default();
                let total_count = *total_counts.get(&key.parent_key).unwrap_or(&0);
                let has_previous_page = start > 0;
                let has_next_page = (start + entities.len() as i64) < total_count;
                results.insert(
                    key,
                    RelationLoadResult {
                        entities,
                        total_count,
                        has_next_page,
                        has_previous_page,
                        offset: start,
                    },
                );
            }

            continue;
        }

        let rendered = render_select_query(
            B::DIALECT,
            &SelectQuery {
                table: T::TABLE_NAME,
                columns: T::column_names()
                    .iter()
                    .map(|column| (*column).to_string())
                    .chain(relation_key_columns)
                    .collect(),
                filter: Some(filter),
                sorts,
                pagination: None,
                count_only: false,
            },
        );

        let rows = B::fetch_rows_with_auth(
            db.pool(),
            &rendered.sql,
            &rendered.values,
            sample.auth_context.as_ref(),
        )
        .await
        .map_err(|error| error.to_string())?;

        let mut grouped_entities: HashMap<RelationKey, Vec<T>> = parent_key_order
            .iter()
            .cloned()
            .map(|parent_key| (parent_key, Vec::new()))
            .collect();

        for row in rows {
            let mut key_parts = Vec::with_capacity(sample.fk_columns.len());
            for index in 0..sample.fk_columns.len() {
                let alias = relation_key_alias(index);
                let part = B::try_get_string(&row, &alias).map_err(|error| error.to_string())?;
                key_parts.push(part);
            }
            let parent_key = RelationKey::new(key_parts);
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

pub struct RelationLoader<T, B: OrmBackend = DefaultBackend> {
    db: crate::db::Database<B>,
    _marker: PhantomData<(T, B)>,
}

impl<T, B: OrmBackend> RelationLoader<T, B> {
    pub fn new(db: crate::db::Database<B>) -> Self {
        Self {
            db,
            _marker: PhantomData,
        }
    }
}

impl<T, B> Loader<String> for RelationLoader<T, B>
where
    B: OrmBackend,
    T: BatchLoadEntity<B>,
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
            use crate::graphql::orm::{SqlDialect, SqlValue};

            if keys.is_empty() {
                return Ok(HashMap::new());
            }

            let backend = B::DIALECT;
            let params = (1..=keys.len())
                .map(|index| backend.placeholder(index))
                .collect::<Vec<_>>()
                .join(", ");
            let batch_column = backend.quote_identifier_path(T::batch_column());
            let sql = format!(
                "SELECT {} FROM {} WHERE {} IN ({})",
                T::column_names().join(", "),
                T::TABLE_NAME,
                batch_column,
                params
            );
            let values = keys
                .iter()
                .cloned()
                .map(SqlValue::String)
                .collect::<Vec<_>>();

            let rows = B::fetch_rows(db.pool(), &sql, &values)
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

impl<T, B> Loader<RelationQueryKey> for RelationLoader<T, B>
where
    B: OrmBackend,
    T: BatchLoadEntity<B>,
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
            let composite_keys = keys
                .iter()
                .map(|key| CompositeRelationQueryKey {
                    relation: key.relation,
                    parent_key: RelationKey::single(key.parent_key.clone()),
                    parent_values: vec![key.parent_value.clone()],
                    fk_columns: vec![key.fk_column],
                    key_part_kinds: vec![RelationKeyPartKind::String],
                    where_signature: key.where_signature.clone(),
                    order_signature: key.order_signature.clone(),
                    page_signature: key.page_signature.clone(),
                    filter: key.filter.clone(),
                    sorts: key.sorts.clone(),
                    pagination: key.pagination.clone(),
                    auth_context: key.auth_context.clone(),
                })
                .collect::<Vec<_>>();
            let composite_results =
                load_composite_relation_keys::<T, B>(db, composite_keys).await?;
            let mut results = HashMap::new();
            for key in keys {
                let composite_key = CompositeRelationQueryKey {
                    relation: key.relation,
                    parent_key: RelationKey::single(key.parent_key.clone()),
                    parent_values: vec![key.parent_value.clone()],
                    fk_columns: vec![key.fk_column],
                    key_part_kinds: vec![RelationKeyPartKind::String],
                    where_signature: key.where_signature.clone(),
                    order_signature: key.order_signature.clone(),
                    page_signature: key.page_signature.clone(),
                    filter: key.filter.clone(),
                    sorts: key.sorts.clone(),
                    pagination: key.pagination.clone(),
                    auth_context: key.auth_context.clone(),
                };
                results.insert(
                    key,
                    composite_results
                        .get(&composite_key)
                        .cloned()
                        .unwrap_or(RelationLoadResult {
                            entities: Vec::new(),
                            total_count: 0,
                            has_next_page: false,
                            has_previous_page: false,
                            offset: 0,
                        }),
                );
            }
            Ok(results)
        }
    }
}

impl<T, B> Loader<CompositeRelationQueryKey> for RelationLoader<T, B>
where
    B: OrmBackend,
    T: BatchLoadEntity<B>,
{
    type Value = RelationLoadResult<T>;
    type Error = String;

    fn load(
        &self,
        keys: &[CompositeRelationQueryKey],
    ) -> impl std::future::Future<
        Output = Result<HashMap<CompositeRelationQueryKey, Self::Value>, Self::Error>,
    > + Send {
        let keys = keys.to_vec();
        let db = self.db.clone();

        async move { load_composite_relation_keys::<T, B>(db, keys).await }
    }
}

/// One Federation entity representation awaiting resolution.
///
/// `_entities` resolves every representation in a fetch concurrently, so the
/// generated entity resolvers enqueue these keys on a shared `DataLoader` and
/// the batch collapses into one statement per entity type and key shape.
#[derive(Clone, Debug)]
pub struct FederationEntityKey {
    /// Stable identity of the declared `@key`, used only for grouping.
    pub key: &'static str,
    /// Backend-quoted key columns in declaration order.
    pub columns: Vec<&'static str>,
    pub key_part_kinds: Vec<RelationKeyPartKind>,
    pub values: Vec<crate::graphql::orm::SqlValue>,
    /// Canonical string form of `values`, matched against the projected row.
    pub representation: RelationKey,
    pub auth_context: Option<DbAuthContext>,
}

impl PartialEq for FederationEntityKey {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
            && self.columns == other.columns
            && self.representation == other.representation
            && self.auth_context.as_ref().map(DbAuthContext::canonical_key)
                == other
                    .auth_context
                    .as_ref()
                    .map(DbAuthContext::canonical_key)
    }
}

impl Eq for FederationEntityKey {}

impl Hash for FederationEntityKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key.hash(state);
        self.columns.hash(state);
        self.representation.hash(state);
        self.auth_context
            .as_ref()
            .map(DbAuthContext::canonical_key)
            .hash(state);
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct FederationGroupKey {
    key: &'static str,
    columns: Vec<&'static str>,
    key_part_kinds: Vec<RelationKeyPartKind>,
    auth_context_key: Option<String>,
}

/// Batches Federation entity lookups for one entity type.
///
/// This loader deliberately does not require [`BatchLoadEntity`]. That trait is
/// written by hand for relation batching and names a single batch column, while
/// a `@key` may span several columns and is generated rather than declared. The
/// bounds here are the ones the query actually needs.
pub struct FederationKeyLoader<T, B: OrmBackend = DefaultBackend> {
    db: crate::db::Database<B>,
    _marker: PhantomData<(T, B)>,
}

impl<T, B: OrmBackend> FederationKeyLoader<T, B> {
    pub fn new(db: crate::db::Database<B>) -> Self {
        Self {
            db,
            _marker: PhantomData,
        }
    }
}

impl<T, B> Loader<FederationEntityKey> for FederationKeyLoader<T, B>
where
    B: OrmBackend,
    T: crate::graphql::orm::DatabaseEntity
        + crate::graphql::orm::FromSqlRow<B>
        + Clone
        + Send
        + Sync
        + 'static,
{
    type Value = Option<T>;
    type Error = String;

    fn load(
        &self,
        keys: &[FederationEntityKey],
    ) -> impl std::future::Future<
        Output = Result<HashMap<FederationEntityKey, Self::Value>, Self::Error>,
    > + Send {
        let keys = keys.to_vec();
        let db = self.db.clone();

        async move {
            use crate::graphql::orm::{SelectQuery, render_select_query};

            if keys.is_empty() {
                return Ok(HashMap::new());
            }

            let mut grouped_keys: HashMap<FederationGroupKey, Vec<FederationEntityKey>> =
                HashMap::new();
            for key in keys {
                let group_key = FederationGroupKey {
                    key: key.key,
                    columns: key.columns.clone(),
                    key_part_kinds: key.key_part_kinds.clone(),
                    auth_context_key: key.auth_context.as_ref().map(DbAuthContext::canonical_key),
                };
                grouped_keys.entry(group_key).or_default().push(key);
            }

            let mut results = HashMap::new();

            for group in grouped_keys.into_values() {
                let sample = group
                    .first()
                    .ok_or_else(|| "federation key batch group should not be empty".to_string())?;

                let representation_values = group
                    .iter()
                    .map(|key| key.values.clone())
                    .collect::<Vec<_>>();
                let filter = relation_key_filter(&sample.columns, &representation_values);

                let key_columns = sample
                    .columns
                    .iter()
                    .zip(sample.key_part_kinds.iter())
                    .enumerate()
                    .map(|(index, (column, kind))| {
                        format!(
                            "{} AS {}",
                            relation_key_projection(B::DIALECT, column, *kind),
                            relation_key_alias(index)
                        )
                    })
                    .collect::<Vec<_>>();

                let rendered = render_select_query(
                    B::DIALECT,
                    &SelectQuery {
                        table: T::TABLE_NAME,
                        columns: T::column_names()
                            .iter()
                            .map(|column| (*column).to_string())
                            .chain(key_columns)
                            .collect(),
                        filter: Some(filter),
                        sorts: Vec::new(),
                        pagination: None,
                        count_only: false,
                    },
                );

                // Routed through the counted helper rather than the backend
                // method so the one-statement-per-batch guarantee is
                // observable through `query_count`.
                let rows = crate::graphql::orm::fetch_rows_with_auth::<B>(
                    db.pool(),
                    &rendered.sql,
                    &rendered.values,
                    sample.auth_context.as_ref(),
                )
                .await
                .map_err(|error| error.to_string())?;

                let mut resolved: HashMap<RelationKey, T> = HashMap::new();
                for row in rows {
                    let mut key_parts = Vec::with_capacity(sample.columns.len());
                    for index in 0..sample.columns.len() {
                        let alias = relation_key_alias(index);
                        let part =
                            B::try_get_string(&row, &alias).map_err(|error| error.to_string())?;
                        key_parts.push(part);
                    }
                    let entity = T::from_row(&row).map_err(|error| error.to_string())?;
                    // A `@key` is validated to be unique, so a second row for the
                    // same representation cannot be distinguished; keep the first.
                    resolved
                        .entry(RelationKey::new(key_parts))
                        .or_insert(entity);
                }

                for key in group {
                    let entity = resolved.get(&key.representation).cloned();
                    results.insert(key, entity);
                }
            }

            Ok(results)
        }
    }
}
