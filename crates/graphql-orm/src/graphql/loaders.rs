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

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
enum RelationValueType {
    String,
    Uuid,
    Int,
    Float,
    Decimal(crate::graphql::orm::DecimalDef),
    Bool,
    Bytes,
    Json,
    UntypedNull,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct RelationKey {
    parts: Vec<String>,
    types: Vec<(RelationValueType, bool)>,
}

impl RelationKey {
    pub fn new(parts: Vec<String>) -> Self {
        let types = vec![(RelationValueType::String, false); parts.len()];
        Self { parts, types }
    }

    pub fn single(part: String) -> Self {
        Self::new(vec![part])
    }

    /// Build a lossless, typed identity from SQL bind values.
    ///
    /// Binary data is hex encoded; nulls retain their SQL type and cannot equal
    /// empty strings or empty byte arrays. This changes pre-v2 cache identities.
    pub fn from_sql_values(values: &[crate::graphql::orm::SqlValue]) -> Self {
        use crate::graphql::orm::SqlValue;
        use RelationValueType as Type;
        let types = values
            .iter()
            .map(|value| match value {
                SqlValue::String(_) => (Type::String, false),
                SqlValue::Uuid(_) => (Type::Uuid, false),
                SqlValue::Int(_) => (Type::Int, false),
                SqlValue::Float(_) => (Type::Float, false),
                SqlValue::Decimal(value) => (Type::Decimal(value.definition()), false),
                SqlValue::Bool(_) => (Type::Bool, false),
                SqlValue::Bytes(_) => (Type::Bytes, false),
                SqlValue::Json(_) => (Type::Json, false),
                SqlValue::StringNull => (Type::String, true),
                SqlValue::UuidNull => (Type::Uuid, true),
                SqlValue::IntNull => (Type::Int, true),
                SqlValue::FloatNull => (Type::Float, true),
                SqlValue::DecimalNull(definition) => (Type::Decimal(*definition), true),
                SqlValue::BoolNull => (Type::Bool, true),
                SqlValue::BytesNull => (Type::Bytes, true),
                SqlValue::JsonNull => (Type::Json, true),
                SqlValue::Null => (Type::UntypedNull, true),
            })
            .collect();
        Self {
            parts: values.iter().map(sql_value_key_part).collect(),
            types,
        }
    }

    /// Interpret non-null SQL text projections using their declared key types.
    ///
    /// Callers must supply one kind per part. Generated code uses this only for
    /// the supported String/UUID/integer/float/boolean relation source types.
    ///
    /// # Panics
    ///
    /// Panics when the number of parts and declared kinds differs.
    pub fn from_projected_parts(mut parts: Vec<String>, kinds: &[RelationKeyPartKind]) -> Self {
        assert_eq!(parts.len(), kinds.len(), "relation key type arity mismatch");
        // SQL Server may render UUID text with a different case than Rust.
        // Normalize the declared UUID domain, keeping ordinary strings exact.
        // Each backend spells a float its own way (`1e+15`, `1.0e+006`,
        // `0.10000000000000001`), so a float is read back as the number and
        // spelled the way `from_sql_values` spells the parent's value.
        for (part, kind) in parts.iter_mut().zip(kinds) {
            match kind {
                RelationKeyPartKind::Uuid => {
                    if let Ok(uuid) = uuid::Uuid::parse_str(part) {
                        *part = uuid.to_string();
                    }
                }
                RelationKeyPartKind::Float => {
                    if let Ok(value) = part.trim().parse::<f64>() {
                        *part = value.to_string();
                    }
                }
                RelationKeyPartKind::String
                | RelationKeyPartKind::Int
                | RelationKeyPartKind::Bool => {}
            }
        }

        let types = kinds
            .iter()
            .map(|kind| {
                (
                    match kind {
                        RelationKeyPartKind::String => RelationValueType::String,
                        RelationKeyPartKind::Uuid => RelationValueType::Uuid,
                        RelationKeyPartKind::Int => RelationValueType::Int,
                        RelationKeyPartKind::Float => RelationValueType::Float,
                        RelationKeyPartKind::Bool => RelationValueType::Bool,
                    },
                    false,
                )
            })
            .collect();
        Self { parts, types }
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
            && RelationKey::from_sql_values(std::slice::from_ref(&self.parent_value))
                == RelationKey::from_sql_values(std::slice::from_ref(&other.parent_value))
            && self.parent_key == other.parent_key
            && self.fk_column == other.fk_column
            && self.where_signature == other.where_signature
            && self.order_signature == other.order_signature
            && self.page_signature == other.page_signature
            && self.auth_context.as_ref().map(DbAuthContext::partition_key)
                == other
                    .auth_context
                    .as_ref()
                    .map(DbAuthContext::partition_key)
    }
}

impl Eq for RelationQueryKey {}

impl Hash for RelationQueryKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.relation.hash(state);
        RelationKey::from_sql_values(std::slice::from_ref(&self.parent_value)).hash(state);
        self.parent_key.hash(state);
        self.fk_column.hash(state);
        self.where_signature.hash(state);
        self.order_signature.hash(state);
        self.page_signature.hash(state);
        self.auth_context
            .as_ref()
            .map(DbAuthContext::partition_key)
            .hash(state);
    }
}

impl PartialEq for CompositeRelationQueryKey {
    fn eq(&self, other: &Self) -> bool {
        self.relation == other.relation
            && RelationKey::from_sql_values(&self.parent_values)
                == RelationKey::from_sql_values(&other.parent_values)
            && self.parent_key == other.parent_key
            && self.fk_columns == other.fk_columns
            && self.key_part_kinds == other.key_part_kinds
            && self.where_signature == other.where_signature
            && self.order_signature == other.order_signature
            && self.page_signature == other.page_signature
            && self.auth_context.as_ref().map(DbAuthContext::partition_key)
                == other
                    .auth_context
                    .as_ref()
                    .map(DbAuthContext::partition_key)
    }
}

impl Eq for CompositeRelationQueryKey {}

impl Hash for CompositeRelationQueryKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.relation.hash(state);
        RelationKey::from_sql_values(&self.parent_values).hash(state);
        self.parent_key.hash(state);
        self.fk_columns.hash(state);
        self.key_part_kinds.hash(state);
        self.where_signature.hash(state);
        self.order_signature.hash(state);
        self.page_signature.hash(state);
        self.auth_context
            .as_ref()
            .map(DbAuthContext::partition_key)
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

/// Generated resolver support. Sanitize policy errors at a relationship
/// boundary with the callback classification every generated path uses.
///
/// An error a policy callback authored itself (it has no source), with or
/// without a host-specific code, is a refusal: `FORBIDDEN`, or
/// `UNAUTHENTICATED` when it carries that code. An error propagated from
/// something else (it has a source, as `?` on a database error leaves), or
/// one that carries `INTERNAL_ERROR` or `SERVICE_UNAVAILABLE`, is a failure
/// and keeps that class. `AUTHORIZATION_MISCONFIGURED` is kept. No authored
/// text or extension crosses the boundary.
#[doc(hidden)]
pub fn relation_authorization_error(error: async_graphql::Error) -> async_graphql::Error {
    use crate::graphql::errors::{OrmErrorCode, OrmPublicError};
    let detail = format!("{error:?}");
    let authored = error.source.is_none();
    let declares_internal = error
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.get("code"))
        == Some(&async_graphql::Value::from(
            OrmErrorCode::InternalError.as_str(),
        ));
    let code = match OrmPublicError::from_callback(error).code {
        code @ (OrmErrorCode::Unauthenticated
        | OrmErrorCode::AuthorizationMisconfigured
        | OrmErrorCode::ServiceUnavailable) => code,
        // An uncoded or host-coded authored error classifies as internal on a
        // repository path; at this boundary it is the callback's refusal.
        OrmErrorCode::InternalError if !authored || declares_internal => {
            OrmErrorCode::InternalError
        }
        _ => OrmErrorCode::Forbidden,
    };
    OrmPublicError::new(code)
        .with_internal(detail)
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

pub(crate) fn relation_selected_fields<'a>(
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
            // Key-less fields are asked too: the field read decision is the
            // same on every generated path.
            let Some(field) = T::repository_field_policies()
                .iter()
                .find(|field| field.api_name == selection.node.name.node.as_str())
            else {
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
    query: crate::graphql::orm::EntityQuery<T, B>,
    auth: Option<&DbAuthContext>,
    visibility: &crate::graphql::orm::ReadVisibility,
    page: crate::graphql::orm::PaginationRequest,
) -> async_graphql::Result<RelationLoadResult<T>>
where
    B: OrmBackend,
    T: BatchLoadEntity<B> + crate::graphql::orm::Entity,
{
    scan_authorized_query(
        db,
        ctx,
        query,
        auth,
        visibility,
        page,
        crate::graphql::orm::EntityAccessSurface::GraphqlRelation,
    )
    .await
}

/// Bounded exact callback pagination shared by generated root and relation reads.
/// A missing budget, unsupported residual filter or incomplete scan fails closed.
#[doc(hidden)]
pub async fn scan_authorized_query<T, B>(
    db: &crate::db::Database<B>,
    ctx: &async_graphql::Context<'_>,
    mut query: crate::graphql::orm::EntityQuery<T, B>,
    auth: Option<&DbAuthContext>,
    visibility: &crate::graphql::orm::ReadVisibility,
    page: crate::graphql::orm::PaginationRequest,
    surface: crate::graphql::orm::EntityAccessSurface,
) -> async_graphql::Result<RelationLoadResult<T>>
where
    B: OrmBackend,
    T: crate::graphql::orm::Entity
        + crate::graphql::orm::FromSqlRow<B>
        + Clone
        + Send
        + Sync
        + 'static,
{
    use crate::graphql::errors::{OrmErrorCode, OrmPublicError};
    use crate::graphql::orm::{PageInput, PaginationConfig};
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
            .any(|clause| crate::graphql::orm::sort_clause_orders_by_column(clause, column))
        {
            query
                .order_clauses
                .push(format!("{} ASC", B::DIALECT.render_identifier_path(column)));
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
                    surface,
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
    auth_context_key: Option<crate::graphql::auth::AuthPartitionKey>,
}

fn sql_value_key_part(value: &crate::graphql::orm::SqlValue) -> String {
    match value {
        crate::graphql::orm::SqlValue::String(value) => value.clone(),
        crate::graphql::orm::SqlValue::Uuid(value) => value.to_string(),
        crate::graphql::orm::SqlValue::Int(value) => value.to_string(),
        crate::graphql::orm::SqlValue::Float(value) => value.to_string(),
        crate::graphql::orm::SqlValue::Decimal(value) => value.value().to_string(),
        crate::graphql::orm::SqlValue::Bool(value) => value.to_string(),
        crate::graphql::orm::SqlValue::Bytes(value) => crate::graphql::orm::binary_key_id(value),
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
            DatabaseBackend::Mssql => crate::graphql::orm::mssql_float_text(column),
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

/// Split one batch group so no statement binds more values than the backend
/// accepts.
///
/// Each key binds `key_arity` values and the statement binds `other_binds`
/// more: everything it carries besides the keys. Every parent's rows are
/// selected, ranked and counted per key, so splitting a group changes how
/// many statements run and nothing about any result. A group that fits, which
/// is every group below the backend's limit, stays one statement.
///
/// # Errors
///
/// When `other_binds` leave no room for even one key, the batch cannot be
/// sent and the caller gets `INVALID_INPUT`; see [`bind_limit_exceeded`].
fn bind_limited_groups<K>(
    dialect: DatabaseBackend,
    group: Vec<K>,
    key_arity: usize,
    other_binds: usize,
) -> Result<Vec<Vec<K>>, String> {
    if group.is_empty() {
        return Ok(Vec::new());
    }
    let key_arity = key_arity.max(1);
    let budget =
        crate::graphql::orm::SqlDialect::max_statement_binds(&dialect).saturating_sub(other_binds);
    let per_statement = budget / key_arity;
    if per_statement == 0 {
        return Err(bind_limit_exceeded());
    }
    if group.len() <= per_statement {
        return Ok(vec![group]);
    }
    let mut groups = Vec::with_capacity(group.len().div_ceil(per_statement));
    let mut keys = group.into_iter().peekable();
    while keys.peek().is_some() {
        groups.push(keys.by_ref().take(per_statement).collect());
    }
    Ok(groups)
}

/// The one check every batched statement passes before it is sent: its
/// rendered bind count against the backend's limit. Group sizes are planned
/// from the counted parts of a statement; this is taken from the statement
/// itself, so a part the plan missed is refused here rather than sent.
fn ensure_within_bind_limit(dialect: DatabaseBackend, bound_values: usize) -> Result<(), String> {
    if bound_values > crate::graphql::orm::SqlDialect::max_statement_binds(&dialect) {
        return Err(bind_limit_exceeded());
    }
    Ok(())
}

/// A batched statement would bind more values than the backend accepts even
/// for a single key: the request's own filter and sort values fill the limit.
/// Reported as `INVALID_INPUT`, the code for a request the client must
/// reduce; the statement is never sent.
fn bind_limit_exceeded() -> String {
    use crate::graphql::errors::{OrmErrorCode, OrmPublicError};
    crate::graphql::errors::loader_error_from_sqlx(crate::graphql::errors::sqlx_error_from_public(
        OrmPublicError::with_message(
            OrmErrorCode::InvalidInput,
            "the request binds more values than one batched statement accepts",
        ),
    ))
}

/// Bound values one relation statement carries besides its parent keys: the
/// relation's filter (which includes any authorization predicate the resolver
/// added), its bound sort values and the two row-number bounds of a page.
fn relation_statement_binds(dialect: DatabaseBackend, sample: &CompositeRelationQueryKey) -> usize {
    let filter = sample.filter.as_ref().map_or(0, |filter| {
        let mut values = Vec::new();
        let mut next_index = 1usize;
        crate::graphql::orm::render_filter_expression(
            dialect,
            filter,
            &mut next_index,
            &mut values,
        );
        values.len()
    });
    let sorts = sample
        .sorts
        .iter()
        .map(|sort| sort.values.len())
        .sum::<usize>();
    filter + sorts + 2
}

fn relation_key_alias(index: usize) -> String {
    format!("__gom_relation_key_{index}")
}

fn render_relation_page(
    dialect: DatabaseBackend,
    query: crate::graphql::orm::SelectQuery,
    primary_keys: &[&str],
    partition_columns: &str,
    sort_sql: &str,
    relation_key_columns: &[String],
    pagination: &crate::graphql::orm::PaginationRequest,
) -> crate::graphql::orm::RenderedQuery {
    use crate::graphql::orm::SqlDialect;

    // Keep payload columns out of the window's sort/materialization. The complete
    // primary key identifies exactly one entity even for composite identities.
    let mut paged_columns = primary_keys
        .iter()
        .enumerate()
        .map(|(index, key)| {
            format!(
                "{} AS __gom_relation_pk_{index}",
                dialect.render_identifier_path(key)
            )
        })
        .chain(relation_key_columns.iter().cloned())
        .collect::<Vec<_>>();
    paged_columns.push(format!(
                "ROW_NUMBER() OVER (PARTITION BY {partition_columns} ORDER BY {sort_sql}) AS __gom_relation_row_number"
            ));

    let rendered_inner = crate::graphql::orm::render_select_query(
        dialect,
        &crate::graphql::orm::SelectQuery {
            table: query.table,
            columns: paged_columns,
            filter: query.filter.clone(),
            sorts: Vec::new(),
            pagination: None,
            count_only: false,
        },
    );

    let start = pagination.offset.max(0);
    let mut row_values = rendered_inner.values.clone();
    let mut row_sql = format!(
        "SELECT * FROM ({}) __gom_relation_page WHERE __gom_relation_row_number > {}",
        rendered_inner.sql,
        dialect.placeholder(row_values.len() + 1)
    );
    row_values.push(crate::graphql::orm::SqlValue::Int(start));
    if let Some(limit) = pagination.limit {
        let end = start.saturating_add(limit.max(0));
        row_sql.push_str(&format!(
            " AND __gom_relation_row_number <= {}",
            dialect.placeholder(row_values.len() + 1)
        ));
        row_values.push(crate::graphql::orm::SqlValue::Int(end));
    }
    // Bounds apply to narrow ranked keys before full rows are fetched. The
    // outer projection restores the original column namespace for order clauses.
    let join = primary_keys
        .iter()
        .enumerate()
        .map(|(index, key)| {
            format!(
                "__gom_relation_entity.{} = __gom_relation_page.__gom_relation_pk_{index}",
                dialect.render_identifier_path(key)
            )
        })
        .collect::<Vec<_>>()
        .join(" AND ");
    let key_aliases = (0..relation_key_columns.len())
        .map(|index| format!("__gom_relation_page.{}", relation_key_alias(index)))
        .collect::<Vec<_>>()
        .join(", ");
    // Entity projections are SQL expressions, including backend casts with
    // output aliases (for example SQL Server MONEY). The narrow page exposes
    // only reserved key/rank aliases, so original entity columns are unambiguous.
    let entity_columns = query.columns.join(", ");
    row_sql = format!(
        "SELECT * FROM (SELECT {entity_columns}, {key_aliases}, __gom_relation_page.__gom_relation_row_number FROM ({row_sql}) __gom_relation_page INNER JOIN {} __gom_relation_entity ON {join}) __gom_relation_rows",
        dialect.render_identifier_path(query.table),
    );
    row_sql.push_str(" ORDER BY ");
    row_sql.push_str(sort_sql);
    row_sql.push_str(", __gom_relation_row_number ASC");

    crate::graphql::orm::RenderedQuery {
        sql: row_sql,
        values: row_values,
    }
}

async fn load_composite_relation_keys<T, B>(
    db: crate::db::Database<B>,
    keys: Vec<CompositeRelationQueryKey>,
    selection: crate::graphql::pagination::ConnectionSelection,
) -> Result<HashMap<CompositeRelationQueryKey, RelationLoadResult<T>>, String>
where
    B: OrmBackend,
    T: BatchLoadEntity<B>,
{
    use crate::graphql::orm::{
        FilterExpression, PaginationRequest, SelectQuery, SortExpression, render_filter_expression,
        render_select_query,
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
            auth_context_key: key.auth_context.as_ref().map(DbAuthContext::partition_key),
        };
        grouped_keys.entry(group_key).or_default().push(key);
    }

    let mut results = HashMap::new();

    let mut groups = Vec::new();
    for group in grouped_keys.into_values() {
        let (arity, other_binds) = group.first().map_or((1, 0), |sample| {
            (
                sample.fk_columns.len(),
                relation_statement_binds(B::DIALECT, sample),
            )
        });
        groups.extend(bind_limited_groups(B::DIALECT, group, arity, other_binds)?);
    }

    for group in groups {
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
                crate::graphql::orm::sort_clause_orders_by_column(&sort.clause, primary_key)
            }) {
                sorts.push(SortExpression::unbound(format!(
                    "{} ASC",
                    B::DIALECT.render_identifier_path(primary_key)
                )));
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

        if sample.pagination.is_some()
            || selection != crate::graphql::pagination::ConnectionSelection::ALL
        {
            let PaginationRequest { limit, offset } =
                sample.pagination.clone().unwrap_or(PaginationRequest {
                    limit: None,
                    offset: 0,
                });
            let partition_columns = sample.fk_columns.join(", ");
            let rendered = render_relation_page(
                B::DIALECT,
                SelectQuery {
                    table: T::TABLE_NAME,
                    columns: T::column_names()
                        .iter()
                        .map(|column| (*column).to_string())
                        .collect(),
                    filter: Some(filter.clone()),
                    sorts: Vec::new(),
                    pagination: None,
                    count_only: false,
                },
                T::PRIMARY_KEYS,
                &partition_columns,
                &sort_sql,
                &relation_key_columns,
                &PaginationRequest {
                    limit: if !selection.count && selection.has_next_page {
                        limit.map(|n| n.saturating_add(1))
                    } else {
                        limit
                    },
                    offset,
                },
            );
            let start = offset.max(0);
            let row_sql = rendered.sql;
            let row_values = rendered.values;
            ensure_within_bind_limit(B::DIALECT, row_values.len())?;

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
            let mut count_sql = format!(
                "SELECT {} FROM {}",
                count_columns.join(", "),
                B::DIALECT.render_identifier_path(T::TABLE_NAME)
            );
            if !where_sql.is_empty() {
                count_sql.push_str(" WHERE ");
                count_sql.push_str(&where_sql);
            }
            count_sql.push_str(" GROUP BY ");
            count_sql.push_str(&partition_columns);
            ensure_within_bind_limit(B::DIALECT, count_values.len())?;

            let needs_rows = selection.rows || (selection.has_next_page && !selection.count);
            let (count_rows, rows) = if selection.count && needs_rows {
                crate::graphql::orm::fetch_rows_pair_with_auth::<B>(
                    db.pool(),
                    &count_sql,
                    &count_values,
                    &row_sql,
                    &row_values,
                    sample.auth_context.as_ref(),
                )
                .await
                .map_err(crate::graphql::errors::loader_error_from_sqlx)?
            } else {
                let count_rows = if selection.count {
                    crate::graphql::orm::fetch_rows_with_auth::<B>(
                        db.pool(),
                        &count_sql,
                        &count_values,
                        sample.auth_context.as_ref(),
                    )
                    .await
                    .map_err(crate::graphql::errors::loader_error_from_sqlx)?
                } else {
                    Vec::new()
                };
                let rows = if needs_rows {
                    crate::graphql::orm::fetch_rows_with_auth::<B>(
                        db.pool(),
                        &row_sql,
                        &row_values,
                        sample.auth_context.as_ref(),
                    )
                    .await
                    .map_err(crate::graphql::errors::loader_error_from_sqlx)?
                } else {
                    Vec::new()
                };
                (count_rows, rows)
            };

            let mut total_counts: HashMap<RelationKey, i64> = parent_key_order
                .iter()
                .cloned()
                .map(|parent_key| (parent_key, 0))
                .collect();
            for row in count_rows {
                let mut key_parts = Vec::with_capacity(sample.fk_columns.len());
                for index in 0..sample.fk_columns.len() {
                    let alias = relation_key_alias(index);
                    let part = B::try_get_string(&row, &alias)
                        .map_err(crate::graphql::errors::loader_error_from_sqlx)?;
                    key_parts.push(part);
                }
                let parent_key =
                    RelationKey::from_projected_parts(key_parts, &sample.key_part_kinds);
                let total = B::try_get_i64(&row, "__gom_relation_total_count")
                    .map_err(crate::graphql::errors::loader_error_from_sqlx)?;
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
                    let part = B::try_get_string(&row, &alias)
                        .map_err(crate::graphql::errors::loader_error_from_sqlx)?;
                    key_parts.push(part);
                }
                let parent_key =
                    RelationKey::from_projected_parts(key_parts, &sample.key_part_kinds);
                let entity =
                    T::from_row(&row).map_err(crate::graphql::errors::loader_error_from_sqlx)?;
                grouped_entities.entry(parent_key).or_default().push(entity);
            }

            for key in group {
                let mut entities = grouped_entities.remove(&key.parent_key).unwrap_or_default();
                let total_count = *total_counts.get(&key.parent_key).unwrap_or(&0);
                let has_previous_page = start > 0;
                let has_next_page = if selection.count {
                    let page_len = if selection.rows {
                        entities.len() as i64
                    } else {
                        limit.map_or((total_count - start).max(0), |n| {
                            n.max(0).min((total_count - start).max(0))
                        })
                    };
                    start.saturating_add(page_len) < total_count
                } else {
                    limit.is_some_and(|n| entities.len() as i64 > n)
                };
                if let Some(limit) = limit {
                    entities.truncate(limit.max(0) as usize);
                }
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

        ensure_within_bind_limit(B::DIALECT, rendered.values.len())?;
        // Count relation batches alongside federation batches so their bounded
        // statement count can be verified through the public ORM counter.
        let rows = crate::graphql::orm::fetch_rows_with_auth::<B>(
            db.pool(),
            &rendered.sql,
            &rendered.values,
            sample.auth_context.as_ref(),
        )
        .await
        .map_err(crate::graphql::errors::loader_error_from_sqlx)?;

        let mut grouped_entities: HashMap<RelationKey, Vec<T>> = parent_key_order
            .iter()
            .cloned()
            .map(|parent_key| (parent_key, Vec::new()))
            .collect();

        for row in rows {
            let mut key_parts = Vec::with_capacity(sample.fk_columns.len());
            for index in 0..sample.fk_columns.len() {
                let alias = relation_key_alias(index);
                let part = B::try_get_string(&row, &alias)
                    .map_err(crate::graphql::errors::loader_error_from_sqlx)?;
                key_parts.push(part);
            }
            let parent_key = RelationKey::from_projected_parts(key_parts, &sample.key_part_kinds);
            let entity =
                T::from_row(&row).map_err(crate::graphql::errors::loader_error_from_sqlx)?;
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
            let batch_column = backend.quote_identifier_path(T::batch_column());
            let mut grouped: HashMap<String, Vec<T>> =
                keys.iter().cloned().map(|key| (key, Vec::new())).collect();

            for keys in bind_limited_groups(backend, keys, 1, 0)? {
                let params = (1..=keys.len())
                    .map(|index| backend.placeholder(index))
                    .collect::<Vec<_>>()
                    .join(", ");
                let sql = format!(
                    "SELECT {} FROM {} WHERE {} IN ({})",
                    T::column_names().join(", "),
                    backend.render_identifier_path(T::TABLE_NAME),
                    batch_column,
                    params
                );
                let values = keys.into_iter().map(SqlValue::String).collect::<Vec<_>>();

                let rows = B::fetch_rows(db.pool(), &sql, &values)
                    .await
                    .map_err(crate::graphql::errors::loader_error_from_sqlx)?;

                for row in rows {
                    let key = T::batch_key_from_row(&row)
                        .map_err(crate::graphql::errors::loader_error_from_sqlx)?;
                    let entity = T::from_row(&row)
                        .map_err(crate::graphql::errors::loader_error_from_sqlx)?;
                    grouped.entry(key).or_default().push(entity);
                }
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
            let composite_results = load_composite_relation_keys::<T, B>(
                db,
                composite_keys,
                crate::graphql::pagination::ConnectionSelection::ALL,
            )
            .await?;
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

        async move {
            load_composite_relation_keys::<T, B>(
                db,
                keys,
                crate::graphql::pagination::ConnectionSelection::ALL,
            )
            .await
        }
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
            && RelationKey::from_sql_values(&self.values)
                == RelationKey::from_sql_values(&other.values)
            && self.columns == other.columns
            && self.key_part_kinds == other.key_part_kinds
            && self.representation == other.representation
            && self.auth_context.as_ref().map(DbAuthContext::partition_key)
                == other
                    .auth_context
                    .as_ref()
                    .map(DbAuthContext::partition_key)
    }
}

impl Eq for FederationEntityKey {}

impl Hash for FederationEntityKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key.hash(state);
        RelationKey::from_sql_values(&self.values).hash(state);
        self.columns.hash(state);
        self.key_part_kinds.hash(state);
        self.representation.hash(state);
        self.auth_context
            .as_ref()
            .map(DbAuthContext::partition_key)
            .hash(state);
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct FederationGroupKey {
    key: &'static str,
    columns: Vec<&'static str>,
    key_part_kinds: Vec<RelationKeyPartKind>,
    auth_context_key: Option<crate::graphql::auth::AuthPartitionKey>,
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
                    auth_context_key: key.auth_context.as_ref().map(DbAuthContext::partition_key),
                };
                grouped_keys.entry(group_key).or_default().push(key);
            }

            let mut results = HashMap::new();

            let mut groups = Vec::new();
            for group in grouped_keys.into_values() {
                let arity = group.first().map_or(1, |sample| sample.columns.len());
                groups.extend(bind_limited_groups(B::DIALECT, group, arity, 0)?);
            }

            for group in groups {
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

                ensure_within_bind_limit(B::DIALECT, rendered.values.len())?;
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
                .map_err(crate::graphql::errors::loader_error_from_sqlx)?;

                let mut resolved: HashMap<RelationKey, T> = HashMap::new();
                for row in rows {
                    let mut key_parts = Vec::with_capacity(sample.columns.len());
                    for index in 0..sample.columns.len() {
                        let alias = relation_key_alias(index);
                        let part = B::try_get_string(&row, &alias)
                            .map_err(crate::graphql::errors::loader_error_from_sqlx)?;
                        key_parts.push(part);
                    }
                    let entity = T::from_row(&row)
                        .map_err(crate::graphql::errors::loader_error_from_sqlx)?;
                    // A `@key` is validated to be unique, so a second row for the
                    // same representation cannot be distinguished; keep the first.
                    resolved
                        .entry(RelationKey::from_projected_parts(
                            key_parts,
                            &sample.key_part_kinds,
                        ))
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

#[cfg(test)]
mod bind_limit_tests {
    use super::*;
    use crate::graphql::orm::{FilterExpression, SqlDialect, SqlValue};

    fn key(filter: Option<FilterExpression>, sort_values: usize) -> CompositeRelationQueryKey {
        let values = vec![SqlValue::Int(1); 3];
        CompositeRelationQueryKey {
            relation: "children",
            parent_key: RelationKey::from_sql_values(&values),
            parent_values: values,
            fk_columns: vec!["a", "b", "c"],
            key_part_kinds: vec![RelationKeyPartKind::Int; 3],
            where_signature: None,
            order_signature: None,
            page_signature: None,
            filter,
            sorts: vec![crate::graphql::orm::SortExpression {
                clause: "rank ASC".into(),
                values: vec![SqlValue::Int(0); sort_values],
            }],
            pagination: None,
            auth_context: None,
        }
    }

    fn binds(count: usize) -> FilterExpression {
        FilterExpression::trusted_fragment(
            vec!["x = ?"; count].join(" AND "),
            vec![SqlValue::Int(1); count],
        )
    }

    #[test]
    fn sql_server_batches_stay_under_the_parameter_limit() {
        // 1,000 parents of a three-column relation bind 3,000 values, above
        // SQL Server's 2,100 parameters.
        let keys = (0..1000).collect::<Vec<i32>>();
        let groups = bind_limited_groups(DatabaseBackend::Mssql, keys.clone(), 3, 5).unwrap();
        assert!(groups.len() > 1);
        assert_eq!(groups.concat(), keys, "every key is loaded once, in order");
        for group in &groups {
            assert!(group.len() * 3 + 5 <= 2_000);
        }
        // The other backends accept the same batch as one statement.
        for dialect in [DatabaseBackend::Sqlite, DatabaseBackend::Postgres] {
            assert_eq!(
                bind_limited_groups(dialect, keys.clone(), 3, 5)
                    .unwrap()
                    .len(),
                1
            );
        }
        assert!(
            bind_limited_groups(DatabaseBackend::Mssql, Vec::<i32>::new(), 3, 5)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn groups_split_exactly_at_the_limit() {
        // 2,000 binds: 5 others leave 1,995, which is 665 three-column keys.
        let sizes = |count: usize| {
            bind_limited_groups(DatabaseBackend::Mssql, vec![0u8; count], 3, 5)
                .unwrap()
                .iter()
                .map(Vec::len)
                .collect::<Vec<_>>()
        };
        assert_eq!(sizes(664), [664]);
        assert_eq!(sizes(665), [665], "exactly at the limit is one statement");
        assert_eq!(sizes(666), [665, 1], "one over the limit is two");
        assert_eq!(sizes(1330), [665, 665]);
        assert_eq!(sizes(1331), [665, 665, 1]);
        // Single-column keys with nothing else bound fill the limit exactly.
        let single = |count: usize| {
            bind_limited_groups(DatabaseBackend::Mssql, vec![0u8; count], 1, 0)
                .unwrap()
                .len()
        };
        assert_eq!(single(2_000), 1);
        assert_eq!(single(2_001), 2);
        // Duplicate keys are kept: each chunk is loaded as it was given.
        assert_eq!(
            bind_limited_groups(DatabaseBackend::Mssql, vec![7, 7, 7], 1, 1_998).unwrap(),
            [vec![7, 7], vec![7]]
        );
    }

    #[test]
    fn a_batch_with_no_room_for_one_key_is_refused_not_sent() {
        // A three-column key needs three binds; 1,998 others leave two.
        let error = bind_limited_groups(DatabaseBackend::Mssql, vec![1, 2], 3, 1_998)
            .expect_err("one key does not fit");
        assert_eq!(error, "graphql-orm-public:INVALID_INPUT");
        assert_eq!(
            bind_limited_groups(DatabaseBackend::Mssql, vec![1, 2], 3, 1_997).unwrap(),
            [vec![1], vec![2]]
        );
        assert!(bind_limited_groups(DatabaseBackend::Mssql, vec![1], 3, 9_000).is_err());
        // The check on the rendered statement itself.
        assert!(ensure_within_bind_limit(DatabaseBackend::Mssql, 2_000).is_ok());
        assert_eq!(
            ensure_within_bind_limit(DatabaseBackend::Mssql, 2_001).unwrap_err(),
            "graphql-orm-public:INVALID_INPUT"
        );
        // Headroom below SQL Server's 2,100 parameters for the RPC's own.
        assert!(DatabaseBackend::Mssql.max_statement_binds() + 2 < 2_100);
    }

    #[test]
    fn every_bound_part_of_a_relation_statement_is_counted() {
        let dialect = DatabaseBackend::Mssql;
        assert_eq!(relation_statement_binds(dialect, &key(None, 0)), 2);
        assert_eq!(
            relation_statement_binds(dialect, &key(Some(binds(4)), 0)),
            6
        );
        // Bound sort values are rendered into the unpaged statement too.
        assert_eq!(
            relation_statement_binds(dialect, &key(Some(binds(4)), 3)),
            9
        );

        // The plan and the rendered statement agree for a full chunk.
        let sample = key(Some(binds(40)), 7);
        let other = relation_statement_binds(dialect, &sample);
        let per_statement =
            bind_limited_groups(dialect, vec![(); 5_000], 3, other).unwrap()[0].len();
        let parents = vec![vec![SqlValue::Int(1); 3]; per_statement];
        let rendered = crate::graphql::orm::render_select_query(
            dialect,
            &crate::graphql::orm::SelectQuery {
                table: "children",
                columns: vec!["id".into()],
                filter: Some(FilterExpression::And(vec![
                    relation_key_filter(&sample.fk_columns, &parents),
                    sample.filter.clone().unwrap(),
                ])),
                sorts: sample.sorts.clone(),
                pagination: None,
                count_only: false,
            },
        );
        assert_eq!(rendered.values.len(), per_statement * 3 + other - 2);
        assert!(ensure_within_bind_limit(dialect, rendered.values.len()).is_ok());
        assert!(rendered.values.len() + 3 > dialect.max_statement_binds() - 2);
    }

    #[test]
    fn float_relation_keys_match_on_every_backend_spelling() {
        let parent = RelationKey::from_sql_values(&[SqlValue::Float(1_000_000.0)]);
        // PostgreSQL, SQL Server style 3 and SQLite `%.17g` spellings.
        for text in ["1e+06", "1.0000000000000000e+006", "1000000"] {
            assert_eq!(
                RelationKey::from_projected_parts(vec![text.into()], &[RelationKeyPartKind::Float]),
                parent,
                "{text}"
            );
        }
        let tenth = RelationKey::from_sql_values(&[SqlValue::Float(0.1)]);
        for text in ["0.10000000000000001", "1.0000000000000001e-001", "0.1"] {
            assert_eq!(
                RelationKey::from_projected_parts(vec![text.into()], &[RelationKeyPartKind::Float]),
                tenth,
                "{text}"
            );
        }
        assert_eq!(
            relation_key_projection(
                DatabaseBackend::Mssql,
                "[ratio]",
                RelationKeyPartKind::Float
            ),
            "CONVERT(NVARCHAR(40), CAST([ratio] AS FLOAT), 3)"
        );
    }
}

#[cfg(test)]
mod relation_page_tests {
    use super::*;
    use crate::graphql::orm::{PaginationRequest, SelectQuery, SqlValue};

    fn page(dialect: DatabaseBackend) -> crate::graphql::orm::RenderedQuery {
        render_relation_page(
            dialect,
            SelectQuery {
                table: "items",
                columns: [
                    "tenant",
                    "id",
                    "owner",
                    "rank",
                    "CAST(payload AS VARCHAR(100)) AS payload",
                ]
                .map(str::to_owned)
                .to_vec(),
                filter: Some(relation_key_filter(
                    &["tenant", "owner"],
                    &[
                        vec![SqlValue::Int(1), SqlValue::Int(2)],
                        vec![SqlValue::Int(1), SqlValue::Int(3)],
                    ],
                )),
                sorts: vec![],
                pagination: None,
                count_only: false,
            },
            &["tenant", "id"],
            "tenant, owner",
            "tenant ASC, owner ASC, rank DESC, id ASC",
            &[
                format!(
                    "{} AS __gom_relation_key_0",
                    relation_key_projection(dialect, "tenant", RelationKeyPartKind::Int)
                ),
                format!(
                    "{} AS __gom_relation_key_1",
                    relation_key_projection(dialect, "owner", RelationKeyPartKind::Int)
                ),
            ],
            &PaginationRequest {
                limit: Some(1),
                offset: 1,
            },
        )
    }

    #[test]
    fn relation_page_sql_ranks_keys_only_for_all_backends() {
        for dialect in [
            DatabaseBackend::Sqlite,
            DatabaseBackend::Postgres,
            DatabaseBackend::Mssql,
        ] {
            let rendered = page(dialect);
            let window = rendered
                .sql
                .split("ROW_NUMBER()")
                .next()
                .unwrap()
                .rsplit("SELECT ")
                .next()
                .unwrap();
            assert!(!rendered.sql.contains("COUNT(*) OVER"), "{}", rendered.sql);
            // Full entities appear only in the join, outside the ranked subquery.
            assert!(!window.contains("payload"), "{}", rendered.sql);
            assert!(
                rendered.sql.contains(
                    "SELECT tenant, id, owner, rank, CAST(payload AS VARCHAR(100)) AS payload,"
                ),
                "{}",
                rendered.sql
            );
            assert!(rendered.sql.contains("__gom_relation_pk_0"));
            assert!(rendered.sql.contains("__gom_relation_pk_1"));
            assert!(rendered.sql.contains(
                "PARTITION BY tenant, owner ORDER BY tenant ASC, owner ASC, rank DESC, id ASC"
            ));
            assert_eq!(
                rendered.values,
                vec![
                    SqlValue::Int(1),
                    SqlValue::Int(2),
                    SqlValue::Int(1),
                    SqlValue::Int(3),
                    SqlValue::Int(1),
                    SqlValue::Int(2)
                ]
            );
            use crate::graphql::orm::SqlDialect;
            assert!(rendered.sql.contains(&format!(
                "__gom_relation_row_number > {}",
                dialect.placeholder(5)
            )));
            assert!(rendered.sql.contains(&format!(
                "__gom_relation_row_number <= {}",
                dialect.placeholder(6)
            )));
        }
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn relation_page_plan_reads_wide_rows_by_key_after_covering_window() {
        use sqlx::Row;
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE items (tenant INTEGER, id INTEGER, owner INTEGER, rank INTEGER, payload TEXT, PRIMARY KEY (tenant, id))").execute(&pool).await.unwrap();
        sqlx::query("CREATE INDEX relation_order ON items (tenant, owner, rank DESC, id ASC)")
            .execute(&pool)
            .await
            .unwrap();
        let rendered = page(DatabaseBackend::Sqlite);
        let plans = sqlx::query(&format!("EXPLAIN QUERY PLAN {}", rendered.sql))
            .bind(1)
            .bind(2)
            .bind(1)
            .bind(3)
            .bind(1)
            .bind(2)
            .fetch_all(&pool)
            .await
            .unwrap();
        let plan = plans
            .iter()
            .map(|row| row.get::<String, _>("detail"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(plan.contains("COVERING INDEX relation_order"), "{plan}");
        assert!(plan.contains("SEARCH __gom_relation_entity USING INDEX sqlite_autoindex_items_1 (tenant=? AND id=?)"), "{plan}");
        eprintln!("{plan}");
    }
}

/// Selection-aware relation cache key. Requirements are part of the cache identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SelectedRelationQueryKey {
    /// Existing relation query and authorization partition.
    pub query: CompositeRelationQueryKey,
    /// Selected connection components.
    pub selection: crate::graphql::pagination::ConnectionSelection,
}

impl<T, B> Loader<SelectedRelationQueryKey> for RelationLoader<T, B>
where
    B: OrmBackend,
    T: BatchLoadEntity<B>,
{
    type Value = RelationLoadResult<T>;
    type Error = String;
    async fn load(
        &self,
        keys: &[SelectedRelationQueryKey],
    ) -> Result<HashMap<SelectedRelationQueryKey, Self::Value>, Self::Error> {
        let mut groups: HashMap<
            crate::graphql::pagination::ConnectionSelection,
            Vec<CompositeRelationQueryKey>,
        > = HashMap::new();
        for key in keys {
            groups
                .entry(key.selection)
                .or_default()
                .push(key.query.clone());
        }
        let mut result = HashMap::new();
        for (selection, queries) in groups {
            for (query, value) in
                load_composite_relation_keys::<T, B>(self.db.clone(), queries, selection).await?
            {
                result.insert(SelectedRelationQueryKey { query, selection }, value);
            }
        }
        Ok(result)
    }
}
