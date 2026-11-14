#[cfg(feature = "mssql")]
use super::MssqlBackend;
#[cfg(feature = "postgres")]
use super::PostgresBackend;
#[cfg(feature = "sqlite")]
use super::SqliteBackend;
use super::core::{
    ColumnDef, DbAuthContext, DecimalDef, EntityMetadata, IndexDef, RelationMetadata, SchemaPolicy,
    SearchIndexDef, SqlValue,
};
use super::dialect::{DatabaseBackend, SqlDialect, current_backend};
use super::{DefaultBackend, OrmBackend, SqlxBackend};
#[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
use super::{MutationContext, MutationLimit, WriteBackend};
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
    fn check_constraints() -> &'static [super::core::CheckConstraintDef] {
        &[]
    }
}

pub trait EntityRelations {
    fn relation_metadata() -> &'static [RelationMetadata] {
        &[]
    }
}

/// Compile-time declaration of persisted columns eligible for GraphQL relation reads.
///
/// Generated automatically by the entity derives, including metadata-only entities.
/// Hand-written relation targets must explicitly implement this contract: list every
/// persisted physical column (not its Rust or GraphQL alias), with `true` only when
/// it is readable, non-private and not input-only. Omitted columns fail closed.
/// Names are unquoted physical identifiers; the relation's backend determines SQL
/// case matching. Never mark a hidden column readable to bypass a relation check.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "relation target `{Self}` has no generated column-readability metadata",
    label = "readable relations require target column-readability metadata",
    note = "derive GraphQLEntity or GraphQLSchemaEntity with current graphql-orm macros, or explicitly implement GeneratedRelationReadability for a hand-written target"
)]
pub trait GeneratedRelationReadability {
    /// Every persisted physical column and its static GraphQL readability.
    const COLUMNS: &'static [(&'static str, bool)];
}

/// Check a generated relation's physical target column at compile time.
///
/// Unknown columns and any unreadable matching declaration fail closed. PostgreSQL
/// uses case-sensitive normalized SQL names; SQLite and MSSQL fold ASCII case.
#[doc(hidden)]
pub const fn generated_relation_column_is_readable(
    columns: &[(&str, bool)],
    column: &str,
    case_sensitive: bool,
) -> bool {
    let mut found = false;
    let mut index = 0;
    while index < columns.len() {
        let (name, readable) = columns[index];
        let left = name.as_bytes();
        let right = column.as_bytes();
        let mut equal = left.len() == right.len();
        let mut byte = 0;
        while equal && byte < left.len() {
            let mut a = left[byte];
            let mut b = right[byte];
            if !case_sensitive {
                a = a.to_ascii_lowercase();
                b = b.to_ascii_lowercase();
            }
            equal = a == b;
            byte += 1;
        }
        if equal {
            if !readable {
                return false;
            }
            found = true;
        }
        index += 1;
    }
    found
}

pub trait DatabaseSearchSchema {
    fn search_index() -> Option<&'static SearchIndexDef> {
        None
    }
}

/// Repository authorization metadata for one persisted field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepositoryFieldPolicyDef {
    /// Rust field identifier used by generated projections.
    pub rust_name: &'static str,
    /// Public API/policy field identity after configured naming rules.
    pub api_name: &'static str,
    /// Optional repository read-policy key.
    pub read_policy: Option<&'static str>,
    /// Optional repository write-policy key.
    pub write_policy: Option<&'static str>,
    /// Whether generated diagnostics and debug output must redact the value.
    pub sensitive: bool,
}

/// Fixed GraphQL name of the read-only field that every entity with a managed
/// record version exposes. It never follows the selected field case, so a
/// client recognises a type's record version by this name alone.
pub const RECORD_VERSION_FIELD: &str = "RecordVersion";

pub trait Entity: DatabaseEntity + DatabaseSchema + EntityRelations + DatabaseSearchSchema {
    fn entity_name() -> &'static str;
    fn metadata() -> &'static EntityMetadata;

    /// Whether the entity's declaration records an explicit policy decision:
    /// an entity `read_policy` or `write_policy`, an entity-level
    /// `auth = "none"`, or the `policy = "open"` marker that states it is
    /// deliberately open to every subject the schema's `auth` setting admits. `schema_roots!` checks it at compile
    /// time under `require_policies: true`. It changes nothing at run time.
    /// Handwritten implementations default to `false`.
    const POLICY_DECISION_DECLARED: bool = false;

    /// Public field name of the entity's managed record version, if it has one.
    ///
    /// Generated entities with `#[graphql_orm(version = "managed")]` return the
    /// declared field's GraphQL name; the same value is also exposed as
    /// [`RECORD_VERSION_FIELD`]. CAS-only versions return none.
    fn managed_version_field() -> Option<&'static str> {
        None
    }

    /// Returns optional public semantic descriptions emitted by the derive.
    /// Handwritten implementations remain source-compatible and return none.
    fn graphql_semantic_metadata() -> Option<&'static super::GraphqlEntitySemanticMetadata> {
        None
    }

    /// Persisted field policy metadata used by non-GraphQL repository paths.
    fn repository_field_policies() -> &'static [RepositoryFieldPolicyDef] {
        &[]
    }

    /// Subscription availability and read policy for one public field identity.
    /// Unknown fields fail closed during event preflight. Derives also include
    /// relation availability and subscription-specific scalar restrictions.
    fn subscription_field_policy(
        field: &str,
    ) -> Option<(&'static str, bool, Option<&'static str>)> {
        Self::repository_field_policies()
            .iter()
            .find(|definition| definition.api_name == field)
            .map(|definition| (definition.api_name, true, definition.read_policy))
    }

    /// Type-erased record used by repository row/field policy callbacks.
    ///
    /// The source-compatible default supplies no record. Generated
    /// repository-only entities override it; callers then fail closed if an
    /// application row policy requires a record from a third-party entity that
    /// has not opted into this capability.
    fn repository_policy_record(&self) -> Option<&(dyn Any + Send + Sync)> {
        None
    }

    /// Authorize fields supplied by a generated repository create input.
    #[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
    fn authorize_repository_create_fields<'a, B: OrmBackend>(
        _db: &'a crate::db::Database<B>,
        _input: &'a (dyn Any + Send + Sync),
    ) -> futures::future::BoxFuture<'a, crate::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    /// Authorize fields supplied by a generated repository update input.
    #[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
    fn authorize_repository_update_fields<'a, B: OrmBackend>(
        _db: &'a crate::db::Database<B>,
        _existing: Option<&'a (dyn Any + Send + Sync)>,
        _input: &'a (dyn Any + Send + Sync),
    ) -> futures::future::BoxFuture<'a, crate::Result<()>> {
        Box::pin(async { Ok(()) })
    }
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
    fn from_row(row: &B::Row) -> crate::Result<Self>;
}

/// A macro-generated, repository-only typed subset of one managed entity.
///
/// Implementations are generated by `#[graphql_orm(projection(...))]`. The
/// projection DTO is decoded directly from a `SELECT` containing only
/// [`Self::COLUMNS`]; the full entity is never materialized.
pub trait ReadProjection<B: OrmBackend = DefaultBackend>:
    FromSqlRow<B> + Clone + Send + Sync + 'static
{
    type Entity: Entity;
    type Filter: DatabaseFilter + Clone + Send + Sync + 'static;
    type Order: DatabaseOrderBy;

    const COLUMNS: &'static [&'static str];
    /// Rust field identities selected by this projection, in decode order.
    const FIELD_NAMES: &'static [&'static str] = &[];
    /// Generated identity for validating SQL visibility without fetching an entity.
    /// Handwritten projections remain compatible; SQL-complete visibility requires
    /// opting into the associated entity identity. Unrestricted reads need no token.
    #[doc(hidden)]
    fn entity_type_id() -> Option<std::any::TypeId> {
        None
    }
    #[doc(hidden)]
    const UNIQUE_COLUMNS: &'static [&'static str];
}

pub trait DatabaseFilter {
    /// Validate a filter before any database work.
    ///
    /// Generated filters use this checked seam for constraints that cannot be
    /// represented by the historical infallible SQL-rendering interface.
    fn validate(&self) -> crate::Result<()> {
        Ok(())
    }

    fn to_sql_conditions(&self) -> (Vec<String>, Vec<SqlValue>);
    fn is_empty(&self) -> bool;

    /// Stable caller-visible filter intent for keyset cursor binding.
    /// This must exclude private authorization predicates and policy parameters.
    /// Handwritten filters retain their existing cursor behavior by default.
    fn keyset_filter_identity(&self) -> Option<String> {
        None
    }

    /// Whether this filter carries request-authorized non-scalar predicates.
    /// Generated relation filters supply this evidence only after preparation.
    fn has_request_authorized_predicates(&self) -> bool {
        false
    }

    /// Public field identities referenced by active predicates.
    ///
    /// Generated filters override this so aggregate execution can authorize
    /// every predicate field before the database observes it. Handwritten
    /// filters must provide field identities or request authorization evidence.
    fn referenced_fields(&self) -> Vec<&'static str> {
        Vec::new()
    }

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

    fn matches_entity(&self, _entity: &(dyn Any + Send + Sync)) -> crate::Result<bool> {
        Ok(true)
    }

    /// Evaluate an in-memory predicate using one query-scoped calendar date.
    #[doc(hidden)]
    fn matches_entity_at(
        &self,
        entity: &(dyn Any + Send + Sync),
        _calendar_today: chrono::NaiveDate,
    ) -> crate::Result<bool> {
        self.matches_entity(entity)
    }

    fn to_filter_expression(&self) -> Option<FilterExpression> {
        let (conditions, values) = self.to_sql_conditions();
        filter_expression_from_raw_parts(&conditions, &values)
    }

    /// Validate and convert this filter to the backend-neutral query IR.
    fn to_filter_expression_checked(&self) -> crate::Result<Option<FilterExpression>> {
        self.validate()?;
        Ok(self.to_filter_expression())
    }
}

fn invalid_filter_input() -> sqlx::Error {
    crate::graphql::errors::sqlx_error_from_public(
        crate::graphql::errors::OrmPublicError::new(
            crate::graphql::errors::OrmErrorCode::InvalidInput,
        )
        .with_internal("generated filter validation failed"),
    )
}

pub trait DatabaseOrderBy {
    fn to_sql_order(&self) -> Option<String>;

    fn requires_context(&self) -> bool {
        false
    }

    fn to_sort_expression(&self) -> Option<SortExpression> {
        self.to_sql_order().map(SortExpression::unbound)
    }

    /// Report whether this order may be rendered on a policy-aware read.
    ///
    /// Generated inputs return an error when an `order_aggregate` term is set
    /// but was not authorized for the current request. Every query builder
    /// treats that as invalid input and refuses to execute, so related-row
    /// counts never bypass the target's read visibility.
    fn validate_order(&self) -> crate::Result<()> {
        Ok(())
    }

    /// Public field identities this order reads.
    ///
    /// Generated order inputs report every scalar field they sort by so the
    /// field's read policy is authorized before any SQL. Handwritten orders
    /// keep the source-compatible empty default.
    fn referenced_fields(&self) -> Vec<&'static str> {
        Vec::new()
    }

    /// Resolve an ordering expression whose bind values come from trusted
    /// server context rather than GraphQL input.
    fn to_sort_expression_with_context(
        &self,
        _ctx: &async_graphql::Context<'_>,
    ) -> async_graphql::Result<Option<SortExpression>> {
        Ok(self.to_sort_expression())
    }
}

/// One orderable field of a generated entity, for ordering with explicit
/// precedence.
///
/// The entity derives generate `<Entity>OrderField`, an enum with one variant
/// per field of `<Entity>OrderByInput`, and implement this trait for it. A
/// field and a direction make the single-field order input that
/// `order_by` accepts, so `order_by_fields([(Field::A, Asc), (Field::B, Asc)])`
/// equals `.order_by({a: Asc}).order_by({b: Asc})` and sorts by `a` first
/// whatever the declaration order of the entity's fields.
pub trait DatabaseOrderField: Copy {
    /// The generated order input this field belongs to.
    type OrderBy: DatabaseOrderBy;

    /// The order input that sorts by this field only.
    fn order(self, direction: OrderDirection) -> Self::OrderBy;
}

/// Internal bridge used by the generated complex-object composition macro.
///
/// This is public only because macro expansion occurs in the consuming crate.
#[doc(hidden)]
pub trait GeneratedRelationsObject {
    type Object<'a>: crate::async_graphql::ObjectType
    where
        Self: 'a;

    /// GraphQL names of the generated identity fields (`NodeId`, `RecordVersion`)
    /// that the composed object serves.
    const GENERATED_IDENTITY_FIELDS: &'static [&'static str];

    fn generated_relations_object(&self) -> Self::Object<'_>;
}

/// Whether `names` contains `name`, usable in constant evaluation.
#[doc(hidden)]
pub const fn generated_field_names_contain(names: &[&str], name: &str) -> bool {
    let mut index = 0;
    while index < names.len() {
        let candidate = names[index].as_bytes();
        let wanted = name.as_bytes();
        if candidate.len() == wanted.len() {
            let mut byte = 0;
            while byte < wanted.len() && candidate[byte] == wanted[byte] {
                byte += 1;
            }
            if byte == wanted.len() {
                return true;
            }
        }
        index += 1;
    }
    false
}

/// Internal proof that `GraphQLRelations` mounted the generated identity fields.
///
/// `GraphQLEntity` requires it for a `#[graphql(complex)]` entity with `node_id`
/// whose complex object it does not own, so a resolvable `NodeId` key is never
/// declared without its field.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "`{Self}` declares `node_id` on a `#[graphql(complex)]` SimpleObject entity, but nothing adds the `NodeId` field to its complex object",
    label = "`NodeId` is not mounted on this complex object",
    note = "add `#[graphql_orm(compose_complex_object)]` to the entity and replace `#[ComplexObject]` with `#[graphql_orm::graphql_complex_object]` on its handwritten impl; the generated `NodeId` and `RecordVersion` are then composed with the handwritten fields",
    note = "an entity with no handwritten complex fields can instead derive `GraphQLRelations`, which adds them to the complex object it generates"
)]
pub trait GeneratedComplexIdentityFields {}

/// Internal proof that `#[graphql_complex_object]` mounted the composed object.
///
/// `GraphQLEntity` requires it when it owns the composed object of a `node_id`
/// entity, so a plain `#[ComplexObject]` cannot leave the key without its field.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "`{Self}` declares `node_id` and `compose_complex_object`, but no `#[graphql_complex_object]` impl mounts the generated `NodeId`",
    label = "`NodeId` is not mounted on this complex object",
    note = "replace `#[ComplexObject]` with `#[graphql_orm::graphql_complex_object]` on the entity's handwritten impl"
)]
pub trait GeneratedComplexObjectMounted {}

/// Backend-neutral, policy-aware query builder for macro-generated repository entities.
///
/// Unlike the legacy pool-bound compatibility builders, this type is bound to a
/// [`Database`](crate::db::Database), applies configured entity and row policy
/// decisions on the repository surface, and bounds every read by the
/// database's maximum page limit before executing SQL.
///
/// The bound never truncates silently: an explicit [`Self::limit`] above the
/// maximum, or an unlimited [`Self::fetch_all`] that matches more rows than the
/// maximum, fails with `PAGE_LIMIT_EXCEEDED`.
///
/// When the row policy decides rows in Rust (a row policy is installed and its
/// Repository-surface decision is `CallbackOnly` or `Prefilter`; a `Complete`
/// predicate is added to the SQL and keeps one query),
/// [`Self::limit`] and [`Self::offset`] count visible rows: the read scans
/// candidate batches in order until the page is complete, reading at most
/// [`PaginationConfig::max_scanned_rows`] candidates, and fails with
/// `PAGE_LIMIT_EXCEEDED` rather than return a short page.
pub struct RepositoryQuery<'a, T, F, O, B: OrmBackend = DefaultBackend> {
    db: &'a crate::db::Database<B>,
    auth: Option<DbAuthContext>,
    query: EntityQuery<T, B>,
    limit: Option<i64>,
    offset: i64,
    max_limit: Option<i64>,
    max_scanned_rows: Option<i64>,
    _inputs: PhantomData<(F, O)>,
}

/// Which repository read is being served.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RepositoryRead {
    All,
    First,
    OptionalOne,
}

impl<'a, T, F, O, B> RepositoryQuery<'a, T, F, O, B>
where
    B: OrmBackend,
    T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
    F: DatabaseFilter + Clone + Send + Sync + 'static,
    O: DatabaseOrderBy,
{
    /// Create a bounded repository query associated with `db`.
    pub fn new(db: &'a crate::db::Database<B>) -> Self {
        Self {
            db,
            auth: None,
            query: EntityQuery::new(),
            limit: None,
            offset: 0,
            max_limit: None,
            max_scanned_rows: None,
            _inputs: PhantomData,
        }
    }

    /// Apply backend-neutral transaction-style database authorization to this read.
    pub fn with_auth(mut self, auth: Option<&DbAuthContext>) -> Self {
        self.auth = auth.cloned();
        self
    }

    /// Apply one generated typed filter.
    pub fn filter(mut self, filter: F) -> Self {
        self.query = self.query.filter_with_entity_matching(&filter);
        self
    }

    /// Add one generated deterministic ordering input.
    ///
    /// Calls compose in call order: the first call is the primary sort key.
    /// One input value with several fields set sorts in the entity's field
    /// **declaration** order, not in the order the fields are written, because
    /// a struct literal carries no field order. Use one call per field, or
    /// [`Self::order_by_fields`], when precedence matters.
    pub fn order_by(mut self, order: O) -> Self {
        self.query = self.query.order_by(&order);
        self
    }

    /// Add ordering terms in the order given: the first pair is the primary
    /// sort key. Each pair is one generated orderable field and a direction,
    /// and is applied exactly as one [`Self::order_by`] call, so the same
    /// orderable-field and read-policy checks apply and it composes with
    /// other ordering calls in call order.
    ///
    /// Use this, or chained `order_by` calls, when precedence matters: one
    /// generated `OrderByInput` value with several fields set always sorts in
    /// the entity's field declaration order.
    pub fn order_by_fields<K>(
        mut self,
        fields: impl IntoIterator<Item = (K, OrderDirection)>,
    ) -> Self
    where
        K: DatabaseOrderField<OrderBy = O>,
    {
        for (field, direction) in fields {
            self = self.order_by(field.order(direction));
        }
        self
    }

    /// Apply the entity's declared default ordering.
    pub fn default_order(mut self) -> Self {
        self.query = self.query.default_order();
        self
    }

    /// Read at most `limit` rows.
    ///
    /// The limit is exact: it is never reduced to the database's maximum page
    /// limit, so a `limit(bound + 1)` overflow check sees the extra row. A
    /// limit above [`PaginationConfig::max_limit`] makes the fetch fail with
    /// `PAGE_LIMIT_EXCEEDED`, and a negative limit with `INVALID_INPUT`; see
    /// [`PaginationConfig::checked_explicit_limit`].
    pub fn limit(mut self, limit: i64) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Override the database's maximum page limit for this read only.
    ///
    /// Trusted internal reads that carry their own bound use this instead of
    /// raising the maximum for every caller of the database. The read stays
    /// bounded: a [`Self::limit`] above `maximum` still fails with
    /// `PAGE_LIMIT_EXCEEDED`, and an unlimited `fetch_all` fails closed when
    /// more than `maximum` rows match. Never pass a value a client chose
    /// without validating it first. A negative `maximum` is treated as `0`.
    pub fn with_max_limit(mut self, maximum: i64) -> Self {
        self.max_limit = Some(maximum.max(0));
        self
    }

    /// Override the row scan budget for this read only; see
    /// [`PaginationConfig::max_scanned_rows`].
    ///
    /// It applies only when the row policy decides rows in Rust. A negative
    /// `budget` is treated as `0`.
    pub fn with_max_scanned_rows(mut self, budget: i64) -> Self {
        self.max_scanned_rows = Some(budget.max(0));
        self
    }

    /// Skip `offset` matching rows. A negative offset is treated as `0`.
    ///
    /// When the row policy decides rows in Rust, the skipped rows are visible
    /// rows.
    pub fn offset(mut self, offset: i64) -> Self {
        self.offset = offset.max(0);
        self
    }

    async fn authorize(self) -> crate::Result<(Self, &'static EntityMetadata)> {
        let metadata = T::metadata();
        self.db
            .ensure_entity_access(
                None,
                metadata.entity_name,
                metadata.read_policy,
                super::core::EntityAccessKind::Read,
                super::core::EntityAccessSurface::Repository,
            )
            .await
            .map_err(crate::graphql::errors::sqlx_error_from_graphql)?;
        Ok((self, metadata))
    }

    async fn row_is_readable(&self, metadata: &EntityMetadata, row: &T) -> crate::Result<bool> {
        self.db
            .can_read_row(
                None,
                metadata.entity_name,
                metadata.read_policy,
                super::core::EntityAccessSurface::Repository,
                row as &(dyn Any + Send + Sync),
            )
            .await
            .map_err(crate::graphql::errors::sqlx_error_from_graphql)
    }

    async fn ensure_fields_readable(
        &self,
        metadata: &EntityMetadata,
        row: &T,
    ) -> crate::Result<()> {
        for field in T::repository_field_policies() {
            self.db
                .ensure_repository_readable_field(
                    None,
                    metadata.entity_name,
                    field.api_name,
                    field.read_policy,
                    Some(row as &(dyn Any + Send + Sync)),
                )
                .await
                .map_err(crate::graphql::errors::sqlx_error_from_public)?;
        }
        Ok(())
    }

    /// Read one SQL window and keep its readable rows. Also reports whether
    /// the row check refused any row of the window, and how many rows the
    /// window read.
    async fn fetch_window(
        &self,
        metadata: &EntityMetadata,
        bound: RepositoryBound,
        offset: i64,
    ) -> crate::Result<(Vec<T>, bool, usize)> {
        // The bound is resolved by the caller, so the SQL read applies it exactly.
        let (rows, read) = if self.query.requires_in_memory_filtering() {
            // As on the plain path, every candidate is read and the residual
            // in-memory predicates filter them before the page is sliced; the
            // candidate count is kept for a later rescan's budget.
            let candidates = self
                .query
                .fetch_candidates_with_auth(self.db, self.auth.as_ref(), 0, None)
                .await?;
            let read = candidates.len();
            (
                slice_matching_candidates(candidates, bound.sql_limit(), offset),
                read,
            )
        } else {
            let rows = self
                .query
                .clone()
                .paginate(&PageInput {
                    limit: bound.sql_limit(),
                    offset: Some(offset),
                })
                .fetch_all_with_auth_and_pagination(
                    self.db,
                    self.auth.as_ref(),
                    PaginationConfig::unbounded(),
                )
                .await?;
            let read = rows.len();
            (rows, read)
        };
        bound.check_overflow(rows.len())?;
        let mut visible = Vec::with_capacity(rows.len());
        let mut refused = false;
        for row in rows {
            if self.row_is_readable(metadata, &row).await? {
                self.ensure_fields_readable(metadata, &row).await?;
                visible.push(row);
            } else {
                refused = true;
            }
        }
        Ok((visible, refused, read))
    }

    async fn fetch_visible(self, read: RepositoryRead) -> crate::Result<Vec<T>> {
        let (mut this, metadata) = self.authorize().await?;
        // Resolve the read decision once. Its predicate narrows the base query,
        // so the initial SQL window and the scan both carry it.
        let mode = repository_read_plan::<T, B>(this.db, Some(&mut this.query)).await?;
        let config = this
            .db
            .pagination_config()
            .with_read_maximum(this.max_limit)
            .with_read_scan_budget(this.max_scanned_rows);
        let bound = match read {
            RepositoryRead::All => config.repository_bound(this.limit)?,
            RepositoryRead::First => config.repository_scan_bound(this.limit)?,
            // Cardinality checks always use one bounded look-ahead row. This
            // must not be clamped to a configured maximum of one, or a second
            // matching row could be mistaken for a unique result.
            RepositoryRead::OptionalOne => RepositoryBound::Exact(2),
        };
        // How many visible rows a bounded read returns.
        let take = match (read, bound) {
            (RepositoryRead::First, RepositoryBound::Exact(limit)) => Some(limit.min(1)),
            (_, RepositoryBound::Exact(limit)) => Some(limit),
            _ => None,
        };
        // An empty page needs no read. Authorization, the read decision and
        // the limit are already checked, so a refused read is still refused.
        if take == Some(0) {
            return Ok(Vec::new());
        }
        // Without a residual check, the read is one window, as it always was.
        // The scan budget applies only once a scan is needed.
        let mut budget = config.max_scanned_rows;
        if mode != RowCheckMode::Residual {
            let (rows, refused, read) = this.fetch_window(metadata, bound, this.offset).await?;
            if !(mode == RowCheckMode::Authoritative && refused && take.is_some()) {
                return Ok(rows);
            }
            // The rescan shares one budget with the window already read.
            budget =
                budget.map(|budget| budget.saturating_sub(i64::try_from(read).unwrap_or(i64::MAX)));
        }

        // One budget covers the whole scan. An unlimited read also keeps its
        // fail-closed maximum, counted before row policy; whichever bound is
        // reached first refuses the read.
        let overflow = match (take, bound) {
            (None, RepositoryBound::FailClosed(maximum)) => Some(maximum),
            _ => None,
        };
        let mut scan =
            VisibleRowScan::new(this.offset, take, budget).with_overflow_after_skip(overflow);
        let mut visible = Vec::new();
        while let Some(batch) = scan.next_batch()? {
            let rows = this
                .query
                .fetch_candidates_with_auth(
                    this.db,
                    this.auth.as_ref(),
                    batch.offset,
                    Some(batch.fetch),
                )
                .await?;
            let fetched = rows.len();
            let mut examined = 0;
            for (row, matches) in rows.into_iter().take(batch.examine) {
                if !scan.wants_more() {
                    break;
                }
                examined += 1;
                scan.count_candidate(matches)?;
                let readable = matches && this.row_is_readable(metadata, &row).await?;
                if scan.record(readable) {
                    this.ensure_fields_readable(metadata, &row).await?;
                    visible.push(row);
                }
            }
            scan.finish_batch(batch, fetched, examined);
        }
        Ok(visible)
    }

    /// Fetch every visible matching row, never a silently truncated subset.
    ///
    /// With [`Self::limit`], at most that many rows are returned. Without one,
    /// at most [`PaginationConfig::max_limit`] rows are read, and the fetch
    /// fails with `PAGE_LIMIT_EXCEEDED` when more rows match; page explicitly
    /// with [`Self::limit`] and [`Self::offset`] to read a larger set. The
    /// configured `default_limit` does not apply to repository reads.
    ///
    /// When the row policy decides rows in Rust, the limit and offset count
    /// visible rows and the read may take more candidate rows than it
    /// returns, up to [`PaginationConfig::max_scanned_rows`] for the whole
    /// scan, with or without a limit; it fails with `PAGE_LIMIT_EXCEEDED`
    /// when that budget runs out before the read is complete.
    ///
    /// The overflow check counts matching rows before row policy is applied,
    /// so it fails safe: a caller can be refused because of rows it cannot
    /// see. The error does not say how many rows matched. With an offset and a
    /// row policy that decides rows in Rust, the count starts after the
    /// skipped visible rows, and the scan budget applies as well; whichever
    /// bound is reached first refuses the read.
    ///
    /// # Errors
    ///
    /// Returns `PAGE_LIMIT_EXCEEDED` as described above, `INVALID_INPUT` for a
    /// negative limit, or a safe authorization, query-rendering, backend, or
    /// row-decoding error. Declared policies without the required provider
    /// fail closed.
    pub async fn fetch_all(self) -> crate::Result<Vec<T>> {
        self.fetch_visible(RepositoryRead::All).await
    }

    /// Fetch the first visible row.
    ///
    /// Reads at most [`Self::limit`] rows, or [`PaginationConfig::max_limit`]
    /// rows without one, and returns the first that passes row policy. More
    /// matching rows are not an error. When the row policy decides rows in
    /// Rust, it returns the first visible row after the [`Self::offset`]
    /// visible rows, scanning within [`PaginationConfig::max_scanned_rows`].
    ///
    /// # Errors
    ///
    /// Returns the same safe errors as [`Self::fetch_all`], except that an
    /// omitted limit fails with `PAGE_LIMIT_EXCEEDED` only when the row scan
    /// budget runs out first.
    pub async fn fetch_first(self) -> crate::Result<Option<T>> {
        Ok(self
            .fetch_visible(RepositoryRead::First)
            .await?
            .into_iter()
            .next())
    }

    /// Fetch zero or one visible row, failing closed if more than one row is visible.
    ///
    /// # Errors
    ///
    /// Returns a cardinality error for multiple visible rows, or the same safe
    /// errors as [`Self::fetch_all`].
    pub async fn fetch_optional_one(self) -> crate::Result<Option<T>> {
        let rows = self.fetch_visible(RepositoryRead::OptionalOne).await?;
        if rows.len() > 1 {
            return Err(sqlx::Error::Protocol(
                "repository optional-one query matched more than one row".to_string(),
            ));
        }
        Ok(rows.into_iter().next())
    }
}

/// SQL filter expression tree used by generated and host-authored queries.
///
/// Prefer typed filter inputs and [`FilterExpression::trusted_fragment`] for
/// host-owned predicates. [`FilterExpression::Raw`] remains for generated
/// compatibility but is a deliberate trusted-SQL surface: never pass
/// unvalidated user input as the clause text.
#[derive(Clone, Debug, PartialEq)]
pub enum FilterExpression {
    /// Legacy trusted raw fragment.
    ///
    /// Prefer [`Self::TrustedFragment`]. The clause must be host- or
    /// macro-generated SQL with placeholders only; values are bound parameters.
    Raw {
        clause: String,
        values: Vec<SqlValue>,
    },
    /// Explicit trusted SQL fragment constructed by host or ORM internals.
    ///
    /// Construction is deliberate: callers must not interpolate user input into
    /// `clause`. Bind all values through `values`.
    TrustedFragment {
        clause: String,
        values: Vec<SqlValue>,
    },
    /// Logical conjunction of child filters.
    And(Vec<FilterExpression>),
    /// Logical disjunction of child filters. An empty list is false; a child
    /// rendering no restriction is true.
    Or(Vec<FilterExpression>),
}

impl FilterExpression {
    /// Construct an explicitly trusted parameterized SQL fragment.
    ///
    /// # Safety contract
    ///
    /// `clause` must not contain unvalidated user input. Identifiers must be
    /// validated/quoted by the caller or dialect helpers before construction.
    pub fn trusted_fragment(clause: impl Into<String>, values: Vec<SqlValue>) -> Self {
        Self::TrustedFragment {
            clause: clause.into(),
            values,
        }
    }

    /// Compatibility constructor for generated raw fragments.
    #[doc(hidden)]
    pub fn raw(clause: impl Into<String>, values: Vec<SqlValue>) -> Self {
        Self::Raw {
            clause: clause.into(),
            values,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SortExpression {
    pub clause: String,
    pub values: Vec<SqlValue>,
}

impl SortExpression {
    pub fn unbound(clause: impl Into<String>) -> Self {
        Self {
            clause: clause.into(),
            values: Vec::new(),
        }
    }
}

/// Whether one term of `clause` orders directly by `column`.
///
/// A term counts only when its expression is exactly the column, optionally
/// qualified and quoted, followed by an optional direction and `NULLS` clause.
/// An expression that merely mentions the column, such as
/// `CASE WHEN id > 0 THEN 0 ELSE 1 END`, does not order by it, so callers that
/// append a primary-key tiebreak still append one.
pub(crate) fn sort_clause_orders_by_column(clause: &str, column: &str) -> bool {
    let expected = unquote_identifier(column);
    split_sort_terms(clause)
        .into_iter()
        .any(|term| sort_term_column(term).is_some_and(|name| name == expected))
}

fn unquote_identifier(identifier: &str) -> &str {
    identifier.trim_matches(|character| matches!(character, '"' | '[' | ']' | '`'))
}

/// Split an `ORDER BY` list on top-level commas, outside parentheses and
/// quoted text.
fn split_sort_terms(clause: &str) -> Vec<&str> {
    let mut terms = Vec::new();
    let mut depth = 0_usize;
    let mut quote: Option<char> = None;
    let mut start = 0;
    for (index, character) in clause.char_indices() {
        match quote {
            Some(open) => {
                let close = if open == '[' { ']' } else { open };
                if character == close {
                    quote = None;
                }
            }
            None => match character {
                '\'' | '"' | '`' | '[' => quote = Some(character),
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                ',' if depth == 0 => {
                    terms.push(&clause[start..index]);
                    start = index + 1;
                }
                _ => {}
            },
        }
    }
    terms.push(&clause[start..]);
    terms
}

/// The unquoted column name of a sort term that is a plain column reference.
///
/// The reference may be qualified, with whitespace around each `.`, and each
/// segment may be bare (`id`), double-quoted (`"id"`, with `""` escapes),
/// bracketed (`[id]`, with `]]` escapes and trailing spaces ignored, as SQL
/// Server does) or backticked. Anything else is an expression.
fn sort_term_column(term: &str) -> Option<String> {
    let mut expression = term.trim();
    let ends_with = |text: &str, suffix: &str| {
        text.len() >= suffix.len()
            && text.is_char_boundary(text.len() - suffix.len())
            && text[text.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
            && text[..text.len() - suffix.len()]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace)
    };
    for suffix in ["NULLS FIRST", "NULLS LAST"] {
        if ends_with(expression, suffix) {
            expression = expression[..expression.len() - suffix.len()].trim_end();
            break;
        }
    }
    for suffix in ["ASC", "DESC"] {
        if ends_with(expression, suffix) {
            expression = expression[..expression.len() - suffix.len()].trim_end();
            break;
        }
    }
    let mut rest = expression;
    loop {
        let (segment, after) = identifier_segment(rest.trim_start())?;
        rest = after.trim_start();
        if rest.is_empty() {
            return Some(segment);
        }
        rest = rest.strip_prefix('.')?;
    }
}

/// Parse one identifier segment from the start of `text`, returning its
/// unquoted name and the remaining text.
fn identifier_segment(text: &str) -> Option<(String, &str)> {
    let quoted = |open: char, close: char| -> Option<(String, &str)> {
        let body = text.strip_prefix(open)?;
        let mut name = String::new();
        let mut characters = body.char_indices().peekable();
        while let Some((index, character)) = characters.next() {
            if character == close {
                if characters.peek().is_some_and(|(_, next)| *next == close) {
                    characters.next();
                    name.push(close);
                    continue;
                }
                return Some((name, &body[index + close.len_utf8()..]));
            }
            name.push(character);
        }
        None
    };
    let (name, rest) = match text.chars().next()? {
        '"' => quoted('"', '"')?,
        '`' => quoted('`', '`')?,
        '[' => {
            let (name, rest) = quoted('[', ']')?;
            (name.trim_end().to_owned(), rest)
        }
        _ => {
            let end = text
                .find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
                .unwrap_or(text.len());
            (text[..end].to_owned(), &text[end..])
        }
    };
    (!name.is_empty()).then_some((name, rest))
}

/// Named bind values returned by an entity-owned computed-order provider.
///
/// GraphQL clients never construct this map. Generated resolvers call the
/// configured server function with the request context, then bind only names
/// declared in the compile-time SQL expression.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OrderExpressionParameters {
    values: std::collections::BTreeMap<String, SqlValue>,
}

impl OrderExpressionParameters {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bind(mut self, name: impl Into<String>, value: SqlValue) -> Self {
        self.values.insert(name.into(), value);
        self
    }

    #[doc(hidden)]
    pub fn require(&self, name: &str) -> async_graphql::Result<SqlValue> {
        self.values.get(name).cloned().ok_or_else(|| {
            async_graphql::Error::new(format!(
                "computed order parameter provider did not supply `{name}`"
            ))
        })
    }
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

/// Portable scalar family for one generated aggregate field.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AggregateFieldKind {
    /// Signed or unsigned integral values stored through the portable integer contract.
    Integral,
    /// Exact fixed-precision decimal values.
    Decimal(DecimalDef),
    /// IEEE floating-point values.
    Floating,
    /// Text and text-rendered comparable scalar values.
    Text,
    /// Boolean values.
    Boolean,
}

/// Closed generated field evidence accepted by the aggregate builder.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AggregateFieldRef {
    /// Stable Rust field identity used by repository policy metadata.
    rust_name: &'static str,
    /// Public GraphQL/policy field identity.
    api_name: &'static str,
    /// Physical column identity emitted only by trusted generated metadata.
    #[doc(hidden)]
    column_name: &'static str,
    /// Portable scalar family.
    kind: AggregateFieldKind,
    /// Whether SQL NULL is possible.
    nullable: bool,
    /// Whether this field may be grouped.
    groupable: bool,
    /// Canonical supported aggregate operators.
    operators: &'static [super::GraphqlAggregateOperator],
}

impl AggregateFieldRef {
    /// Creates immutable evidence emitted by `GraphQLEntity`.
    ///
    /// This is a macro ABI. Applications should use the generated aggregate
    /// field enum instead of constructing field evidence directly.
    #[doc(hidden)]
    pub const fn generated(
        rust_name: &'static str,
        api_name: &'static str,
        column_name: &'static str,
        kind: AggregateFieldKind,
        nullable: bool,
        groupable: bool,
        operators: &'static [super::GraphqlAggregateOperator],
    ) -> Self {
        Self {
            rust_name,
            api_name,
            column_name,
            kind,
            nullable,
            groupable,
            operators,
        }
    }

    /// Public GraphQL field identity.
    pub const fn api_name(&self) -> &'static str {
        self.api_name
    }

    /// Portable scalar family.
    pub const fn kind(&self) -> AggregateFieldKind {
        self.kind
    }

    /// Whether this field can contain SQL `NULL`.
    pub const fn nullable(&self) -> bool {
        self.nullable
    }

    /// Whether this field supports grouping.
    pub const fn groupable(&self) -> bool {
        self.groupable
    }

    /// Canonical supported aggregate operators.
    pub const fn operators(&self) -> &'static [super::GraphqlAggregateOperator] {
        self.operators
    }
}

/// Implemented by macro-generated per-entity aggregate field enums.
pub trait TypedAggregateField<T: Entity>: Copy + Eq + Send + Sync + 'static {
    /// Returns immutable generated field evidence.
    fn aggregate_field(self) -> &'static AggregateFieldRef;
    /// Generated entity identity for validating complete SQL visibility.
    /// Handwritten implementations remain compatible; Complete visibility requires
    /// this token, while explicitly Unrestricted reads need no token.
    #[doc(hidden)]
    fn entity_type_id() -> Option<std::any::TypeId> {
        None
    }
}

/// One requested aggregate expression.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AggregateExpression {
    /// Aggregate operator.
    operator: super::GraphqlAggregateOperator,
    /// Selected field, or `None` for row count.
    field: Option<AggregateFieldRef>,
}

/// One exact decoded grouped or aggregate scalar.
#[derive(Clone, Debug, PartialEq)]
pub enum AggregateValue {
    /// SQL NULL for minimum, maximum, sum, or a nullable grouping key.
    Null,
    /// Non-negative count.
    Count(i64),
    /// Widened signed integral aggregate or grouping value.
    Integral(i128),
    /// Exact fixed-precision decimal.
    Decimal(rust_decimal::Decimal),
    /// Floating-point aggregate or grouping value.
    Floating(f64),
    /// Text or canonically rendered comparable value.
    Text(String),
    /// Boolean grouping value.
    Boolean(bool),
}

/// One named grouping key or aggregate result.
#[derive(Clone, Debug, PartialEq)]
pub struct AggregateResultValue {
    /// Public field name, or `None` for row count.
    pub field: Option<&'static str>,
    /// Aggregate operator for metrics; absent for grouping keys.
    pub operator: Option<super::GraphqlAggregateOperator>,
    /// Decoded value.
    pub value: AggregateValue,
}

/// One deterministically ordered aggregate result group.
#[derive(Clone, Debug, PartialEq)]
pub struct AggregateResultRow {
    /// Grouping values in declaration order.
    pub groups: Vec<AggregateResultValue>,
    /// Aggregate metrics in declaration order.
    pub metrics: Vec<AggregateResultValue>,
}

/// GraphQL aggregate operator used by generated opt-in aggregate inputs.
#[derive(Clone, Copy, Debug, Eq, PartialEq, async_graphql::Enum)]
pub enum GraphqlAggregateInputOperator {
    /// Count matching rows or non-null values.
    Count,
    /// Minimum non-null value.
    Min,
    /// Maximum non-null value.
    Max,
    /// Sum non-null numeric values.
    Sum,
}

impl From<GraphqlAggregateInputOperator> for super::GraphqlAggregateOperator {
    fn from(value: GraphqlAggregateInputOperator) -> Self {
        match value {
            GraphqlAggregateInputOperator::Count => Self::Count,
            GraphqlAggregateInputOperator::Min => Self::Min,
            GraphqlAggregateInputOperator::Max => Self::Max,
            GraphqlAggregateInputOperator::Sum => Self::Sum,
        }
    }
}

/// Portable scalar family exposed by generated aggregate result entries.
#[derive(Clone, Copy, Debug, Eq, PartialEq, async_graphql::Enum)]
pub enum GraphqlAggregateResultKind {
    /// SQL NULL.
    Null,
    /// Non-negative count.
    Count,
    /// Widened integral value serialized exactly as decimal text.
    Integral,
    /// Exact fixed-precision decimal serialized as decimal text.
    Decimal,
    /// Floating-point value serialized using Rust's finite scalar form.
    Floating,
    /// Text value.
    Text,
    /// Boolean value.
    Boolean,
}

/// One typed entry in a generated aggregate result.
///
/// Every `graphql-orm` subgraph with an aggregate entity exports this same
/// object. Federation v2 composition rejects two subgraphs that define the same
/// non-entity object field unless the type is `@shareable`. The type carries no
/// owned data, so, as with `PageInfo`, the directive describes it accurately.
#[derive(Clone, Debug, PartialEq, async_graphql::SimpleObject)]
#[graphql(shareable)]
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[cfg_attr(
    feature = "field-case-screaming-snake",
    graphql(rename_fields = "SCREAMING_SNAKE_CASE")
)]
#[cfg_attr(feature = "field-case-lower", graphql(rename_fields = "lowercase"))]
#[cfg_attr(feature = "field-case-upper", graphql(rename_fields = "UPPERCASE"))]
pub struct GraphqlAggregateResultValue {
    /// Public field name; absent only for row count.
    pub field: Option<String>,
    /// Canonical operator name; absent for group keys.
    pub operator: Option<String>,
    /// Portable scalar family.
    pub kind: GraphqlAggregateResultKind,
    /// Exact bounded scalar representation; absent for SQL NULL.
    pub value: Option<String>,
}

/// One generated aggregate result group.
///
/// Shared by every aggregate-enabled subgraph and therefore `@shareable`, like
/// [`GraphqlAggregateResultValue`].
#[derive(Clone, Debug, PartialEq, async_graphql::SimpleObject)]
#[graphql(shareable)]
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[cfg_attr(
    feature = "field-case-screaming-snake",
    graphql(rename_fields = "SCREAMING_SNAKE_CASE")
)]
#[cfg_attr(feature = "field-case-lower", graphql(rename_fields = "lowercase"))]
#[cfg_attr(feature = "field-case-upper", graphql(rename_fields = "UPPERCASE"))]
pub struct GraphqlAggregateResultRow {
    /// Group keys in requested order.
    pub groups: Vec<GraphqlAggregateResultValue>,
    /// Metrics in requested order.
    pub metrics: Vec<GraphqlAggregateResultValue>,
}

impl From<AggregateResultValue> for GraphqlAggregateResultValue {
    fn from(value: AggregateResultValue) -> Self {
        let (kind, rendered) = match value.value {
            AggregateValue::Null => (GraphqlAggregateResultKind::Null, None),
            AggregateValue::Count(value) => {
                (GraphqlAggregateResultKind::Count, Some(value.to_string()))
            }
            AggregateValue::Integral(value) => (
                GraphqlAggregateResultKind::Integral,
                Some(value.to_string()),
            ),
            AggregateValue::Decimal(value) => {
                (GraphqlAggregateResultKind::Decimal, Some(value.to_string()))
            }
            AggregateValue::Floating(value) => (
                GraphqlAggregateResultKind::Floating,
                Some(value.to_string()),
            ),
            AggregateValue::Text(value) => (GraphqlAggregateResultKind::Text, Some(value)),
            AggregateValue::Boolean(value) => {
                (GraphqlAggregateResultKind::Boolean, Some(value.to_string()))
            }
        };
        Self {
            field: value.field.map(str::to_owned),
            operator: value
                .operator
                .map(|operator| aggregate_operator_sql(operator).to_owned()),
            kind,
            value: rendered,
        }
    }
}

impl From<AggregateResultRow> for GraphqlAggregateResultRow {
    fn from(value: AggregateResultRow) -> Self {
        Self {
            groups: value.groups.into_iter().map(Into::into).collect(),
            metrics: value.metrics.into_iter().map(Into::into).collect(),
        }
    }
}

/// Validated internal grouped aggregate query representation.
#[derive(Clone, Debug, PartialEq)]
struct GroupedAggregateSqlQuery {
    /// Trusted table identity from entity metadata.
    table: &'static str,
    /// Grouping fields in declaration order.
    groups: Vec<AggregateFieldRef>,
    /// Aggregate expressions in declaration order.
    metrics: Vec<AggregateExpression>,
    /// Fully SQL-renderable filter.
    filter: Option<FilterExpression>,
    /// Positive maximum result groups.
    group_limit: i64,
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
    /// Maximum candidate rows one visible-row scan of a Rust repository read
    /// takes from the database, plus one look-ahead row. A read scans when its
    /// row checks run in Rust (a row policy whose Repository-surface decision
    /// is `CallbackOnly` or `Prefilter`, or `Complete` for repository search),
    /// or rescans when the row callback refuses a row of a page its decision
    /// should have served. `limit` and `offset` then count visible rows, and a
    /// scan that would need more candidates fails with `PAGE_LIMIT_EXCEEDED`
    /// rather than return a short page. It does not bound or refuse a read
    /// that needs no scan. Whether the budget runs out depends on
    /// rows the caller cannot see, so set it in trusted code. The same budget
    /// caps the candidates a generated GraphQL bulk write classifies when its
    /// row policy or filter is evaluated in Rust. `None` removes the bound.
    pub max_scanned_rows: Option<i64>,
    /// Maximum rows one generated GraphQL `updateMany` or `deleteMany` field
    /// may write. A bulk write that matches more rows the caller is allowed
    /// to write is refused with `PAGE_LIMIT_EXCEEDED` before any row changes;
    /// it is never truncated. The bound counts rows after visibility: rows
    /// the caller can neither read nor write are not counted, so for a fixed
    /// set of rows the caller may write the outcome does not depend on other
    /// matching rows. When row policy or a filter is evaluated in Rust, the
    /// candidates are classified in bounded pages and the scan is capped by
    /// [`Self::max_scanned_rows`], as for reads. The bound applies to each
    /// bulk mutation field of a request separately. Repository `update_where`
    /// and `delete_where` helpers are trusted code and are not bounded by it.
    /// `None` removes the bound.
    pub max_bulk_write_rows: Option<i64>,
}

impl Default for PaginationConfig {
    fn default() -> Self {
        Self {
            default_limit: Some(Self::DEFAULT_LIMIT),
            max_limit: Some(Self::DEFAULT_MAX_LIMIT),
            max_scanned_rows: Some(Self::DEFAULT_MAX_SCANNED_ROWS),
            max_bulk_write_rows: Some(Self::DEFAULT_MAX_BULK_WRITE_ROWS),
        }
    }
}

impl PaginationConfig {
    /// Secure recommended default connection limit.
    pub const SECURE_DEFAULT_LIMIT: i64 = 50;
    /// Secure recommended maximum connection limit.
    pub const SECURE_MAX_LIMIT: i64 = 100;
    /// Legacy default connection limit retained for migration.
    pub const LEGACY_DEFAULT_LIMIT: i64 = 1000;
    /// Legacy maximum connection limit retained for migration.
    pub const LEGACY_MAX_LIMIT: i64 = 1000;

    /// Default connection limit used when a request omits `PageInput.limit`.
    ///
    /// **Current default:** [`Self::SECURE_DEFAULT_LIMIT`] (`50`).
    /// **Previous default:** [`Self::LEGACY_DEFAULT_LIMIT`] (`1000`).
    /// **Secure recommended:** [`Self::SECURE_DEFAULT_LIMIT`].
    /// Use [`Self::legacy`] to restore the previous behavior during migration.
    pub const DEFAULT_LIMIT: i64 = Self::SECURE_DEFAULT_LIMIT;
    /// Default cap applied to explicit and default limits.
    ///
    /// **Current default:** [`Self::SECURE_MAX_LIMIT`] (`100`).
    /// **Previous default:** [`Self::LEGACY_MAX_LIMIT`] (`1000`).
    pub const DEFAULT_MAX_LIMIT: i64 = Self::SECURE_MAX_LIMIT;
    /// Default row scan budget of a Rust repository read whose row checks run
    /// in Rust; see [`Self::max_scanned_rows`].
    pub const DEFAULT_MAX_SCANNED_ROWS: i64 = 10_000;
    /// Default row bound of a generated GraphQL bulk write; see
    /// [`Self::max_bulk_write_rows`].
    pub const DEFAULT_MAX_BULK_WRITE_ROWS: i64 = 10_000;

    /// Create a config that does not add a default limit and does not cap limits.
    ///
    /// Use only for trusted internal jobs. GraphQL surfaces should keep bounds.
    pub const fn unbounded() -> Self {
        Self {
            default_limit: None,
            max_limit: None,
            max_scanned_rows: None,
            max_bulk_write_rows: None,
        }
    }

    /// Restore the pre-0.3 connection limits (`1000` / `1000`).
    pub const fn legacy() -> Self {
        Self {
            default_limit: Some(Self::LEGACY_DEFAULT_LIMIT),
            max_limit: Some(Self::LEGACY_MAX_LIMIT),
            max_scanned_rows: Some(Self::DEFAULT_MAX_SCANNED_ROWS),
            max_bulk_write_rows: Some(Self::DEFAULT_MAX_BULK_WRITE_ROWS),
        }
    }

    /// Secure recommended limits (`50` / `100`).
    pub const fn secure() -> Self {
        Self {
            default_limit: Some(Self::SECURE_DEFAULT_LIMIT),
            max_limit: Some(Self::SECURE_MAX_LIMIT),
            max_scanned_rows: Some(Self::DEFAULT_MAX_SCANNED_ROWS),
            max_bulk_write_rows: Some(Self::DEFAULT_MAX_BULK_WRITE_ROWS),
        }
    }

    /// Create a config with the default limit disabled but explicit limits capped.
    pub const fn explicit_only(max_limit: i64) -> Self {
        Self {
            default_limit: None,
            max_limit: Some(max_limit),
            max_scanned_rows: Some(Self::DEFAULT_MAX_SCANNED_ROWS),
            max_bulk_write_rows: Some(Self::DEFAULT_MAX_BULK_WRITE_ROWS),
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

    /// Return a copy with a different row scan budget; see
    /// [`Self::max_scanned_rows`].
    pub fn with_max_scanned_rows(mut self, max_scanned_rows: Option<i64>) -> Self {
        self.max_scanned_rows = max_scanned_rows;
        self
    }

    /// Return a copy with a different generated bulk-write row bound; see
    /// [`Self::max_bulk_write_rows`].
    pub fn with_max_bulk_write_rows(mut self, max_bulk_write_rows: Option<i64>) -> Self {
        self.max_bulk_write_rows = max_bulk_write_rows;
        self
    }

    /// Clamp one explicit limit without applying a default.
    ///
    /// This is GraphQL page semantics: an oversized client `page.limit` is
    /// reduced to `max_limit`, and the connection's `pageInfo` reports the
    /// remaining rows. Trusted Rust code that needs an exact bound, such as a
    /// `bound + 1` overflow check, should use [`Self::checked_explicit_limit`]
    /// instead, which refuses rather than reduces.
    pub fn clamp_explicit_limit(&self, limit: Option<i64>) -> Option<i64> {
        limit.map(|limit| self.clamp_limit_value(limit))
    }

    /// Check one trusted explicit row limit against `max_limit` without
    /// changing it.
    ///
    /// The Rust repository builders' `limit(n)` uses this, so a limit is
    /// honoured exactly or refused, never silently reduced. Raise the maximum
    /// with [`Database::with_pagination_config`](crate::db::Database::with_pagination_config)
    /// or `DatabaseBuilder::max_page_limit` when trusted code needs a larger
    /// bound.
    ///
    /// # Errors
    ///
    /// Returns [`OrmErrorCode::PageLimitExceeded`](crate::graphql::errors::OrmErrorCode::PageLimitExceeded)
    /// when `limit` exceeds `max_limit`, and
    /// [`OrmErrorCode::InvalidInput`](crate::graphql::errors::OrmErrorCode::InvalidInput)
    /// when `limit` is negative.
    pub fn checked_explicit_limit(
        &self,
        limit: i64,
    ) -> Result<i64, crate::graphql::errors::OrmPublicError> {
        use crate::graphql::errors::{OrmErrorCode, OrmPublicError};
        if limit < 0 {
            return Err(OrmPublicError::with_message(
                OrmErrorCode::InvalidInput,
                "requested limit must not be negative",
            ));
        }
        match self.max_limit {
            Some(maximum) if limit > maximum.max(0) => Err(OrmPublicError::with_message(
                OrmErrorCode::PageLimitExceeded,
                format!(
                    "requested limit {limit} exceeds the configured maximum page limit {}",
                    maximum.max(0)
                ),
            )),
            _ => Ok(limit),
        }
    }

    /// Apply a per-read maximum set with a builder's `with_max_limit`.
    pub(crate) fn with_read_maximum(mut self, maximum: Option<i64>) -> Self {
        if let Some(maximum) = maximum {
            self.max_limit = Some(maximum);
        }
        self
    }

    /// Apply a per-read scan budget set with a builder's `with_max_scanned_rows`.
    pub(crate) fn with_read_scan_budget(mut self, budget: Option<i64>) -> Self {
        if let Some(budget) = budget {
            self.max_scanned_rows = Some(budget);
        }
        self
    }

    /// Resolve the row bound of one Rust repository read.
    ///
    /// An explicit limit is checked by [`Self::checked_explicit_limit`]. An
    /// omitted limit reads at most `max_limit` rows and fails closed when more
    /// rows match, so an omitted limit never truncates either.
    pub(crate) fn repository_bound(&self, limit: Option<i64>) -> crate::Result<RepositoryBound> {
        match limit {
            Some(limit) => self
                .checked_explicit_limit(limit)
                .map(RepositoryBound::Exact)
                .map_err(crate::graphql::errors::sqlx_error_from_public),
            None => Ok(match self.max_limit {
                Some(maximum) => RepositoryBound::FailClosed(maximum.max(0)),
                None => RepositoryBound::Unbounded,
            }),
        }
    }

    /// Resolve a scan bound that never refuses an overflow: an explicit limit
    /// is checked, an omitted one reads up to `max_limit` rows.
    pub(crate) fn repository_scan_bound(
        &self,
        limit: Option<i64>,
    ) -> crate::Result<RepositoryBound> {
        Ok(match self.repository_bound(limit)? {
            RepositoryBound::FailClosed(maximum) => RepositoryBound::Exact(maximum),
            bound => bound,
        })
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

/// Row bound of one Rust repository read. See
/// [`PaginationConfig::repository_bound`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RepositoryBound {
    /// Read at most this many rows; the caller asked for exactly this bound.
    Exact(i64),
    /// No limit was given: read one row past this maximum and refuse if it arrives.
    FailClosed(i64),
    /// No limit was given and no maximum is configured.
    Unbounded,
}

impl RepositoryBound {
    /// The `LIMIT` to render.
    pub(crate) fn sql_limit(self) -> Option<i64> {
        match self {
            Self::Exact(limit) => Some(limit),
            Self::FailClosed(maximum) => Some(maximum.saturating_add(1)),
            Self::Unbounded => None,
        }
    }

    /// The same bound capped at `cap` rows, for reads that need only a few.
    pub(crate) fn capped(self, cap: i64) -> Self {
        match self {
            Self::Exact(limit) => Self::Exact(limit.min(cap)),
            Self::FailClosed(_) | Self::Unbounded => Self::Exact(cap),
        }
    }

    /// Refuse a fail-closed read whose look-ahead row arrived.
    pub(crate) fn check_overflow(self, rows: usize) -> crate::Result<()> {
        if let Self::FailClosed(maximum) = self
            && i64::try_from(rows).unwrap_or(i64::MAX) > maximum
        {
            use crate::graphql::errors::{OrmErrorCode, OrmPublicError, sqlx_error_from_public};
            // The message names no row count: overflow is counted before row
            // policy, so a count would disclose rows the caller cannot see.
            return Err(sqlx_error_from_public(OrmPublicError::with_message(
                OrmErrorCode::PageLimitExceeded,
                "page limit exceeded; set an explicit limit and offset",
            )));
        }
        Ok(())
    }
}

/// Keep the candidates that satisfy the residual in-memory predicates and
/// slice `offset`/`limit` from them, as in-memory filtering does.
pub(crate) fn slice_matching_candidates<T>(
    candidates: Vec<(T, bool)>,
    limit: Option<i64>,
    offset: i64,
) -> Vec<T> {
    let rows = candidates
        .into_iter()
        .filter_map(|(row, matches)| matches.then_some(row))
        .skip(usize::try_from(offset.max(0)).unwrap_or(usize::MAX));
    match limit {
        Some(limit) => rows
            .take(usize::try_from(limit.max(0)).unwrap_or(usize::MAX))
            .collect(),
        None => rows.collect(),
    }
}

/// How a Rust repository read's row checks relate to its SQL.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RowCheckMode {
    /// No row policy is installed, so the row check gives the same answer for
    /// every row and SQL `LIMIT`/`OFFSET` already count the rows returned.
    Fixed,
    /// The SQL read carries the row authority (`Unrestricted`, or a
    /// `Complete` predicate the read applies to its query). `LIMIT`/`OFFSET` stay in SQL. If
    /// the row callback still refuses a row of a bounded page, the page is
    /// read again as a visible-row scan so it is never short.
    Authoritative,
    /// Rows are decided in Rust, so `limit` and `offset` count visible rows
    /// through a [`VisibleRowScan`].
    Residual,
}

/// Resolve the Repository-surface read decision for `T` once and classify the
/// read's row checks.
///
/// With `query`, the decision's SQL predicate (`Prefilter` or `Complete`) is
/// added to that query, so both the initial SQL window and any visible-row
/// scan carry it, and a `Complete` decision keeps the single SQL path. Without
/// one, the read's executor does not receive the predicate, so the row
/// callback stays the authority and `Complete` is residual for paging.
pub(crate) async fn repository_read_plan<T, B>(
    db: &crate::db::Database<B>,
    query: Option<&mut EntityQuery<T, B>>,
) -> crate::Result<RowCheckMode>
where
    T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
    B: OrmBackend,
{
    let visibility = db
        .read_visibility::<T>(None, super::core::EntityAccessSurface::Repository)
        .await
        .map_err(crate::graphql::errors::sqlx_error_from_graphql)?;
    let applies_predicate = query.is_some();
    if let Some(query) = query {
        *query = query
            .clone()
            .with_read_visibility(&visibility)
            .map_err(crate::graphql::errors::sqlx_error_from_graphql)?;
    }
    Ok(if db.row_policy().is_none() {
        RowCheckMode::Fixed
    } else {
        match visibility {
            super::ReadVisibility::Unrestricted => RowCheckMode::Authoritative,
            super::ReadVisibility::Complete(_) if applies_predicate => RowCheckMode::Authoritative,
            _ => RowCheckMode::Residual,
        }
    })
}

/// One SQL batch of a [`VisibleRowScan`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ScanBatch {
    /// SQL `OFFSET` of the batch, in candidate rows.
    pub(crate) offset: i64,
    /// SQL `LIMIT` of the batch. The last batch the budget allows reads one
    /// row more than it examines, to learn whether more rows remain.
    pub(crate) fetch: i64,
    /// How many fetched rows the scan may examine.
    pub(crate) examine: usize,
}

/// Applies `offset` and `limit` to the visible rows of a read whose row checks
/// run in Rust.
///
/// The scan reads batches of candidate rows in the query's order, checks each
/// row, skips the first `skip` visible rows and collects up to `take` visible
/// rows, or every remaining visible row when `take` is `None`. It reads at
/// most `budget` candidate rows in total, plus one look-ahead row that is
/// never examined, so the budget covers the whole scan. When the budget runs out before the page is
/// complete and more rows remain, [`Self::next_batch`] fails with
/// `PAGE_LIMIT_EXCEEDED` instead of letting the read return a short page. The
/// error names no row count.
#[derive(Debug)]
pub(crate) struct VisibleRowScan {
    skip: u64,
    take: Option<u64>,
    budget: Option<u64>,
    position: u64,
    batch: u64,
    collected: u64,
    exhausted: bool,
    /// Whether a look-ahead row has already shown that rows remain past the
    /// budget.
    looked_ahead: bool,
    /// An unlimited read's `max_limit`: matching rows after the skipped ones
    /// are counted before row policy, and more than this many fail closed.
    overflow_after_skip: Option<u64>,
    matched_after_skip: u64,
}

impl VisibleRowScan {
    /// Batches grow up to this size while visible rows are sparse.
    const MAX_GROWN_BATCH: u64 = 1_000;

    /// Skip `skip` visible rows, then collect `take` of them, or all of the
    /// remaining visible rows when `take` is `None`.
    pub(crate) fn new(skip: i64, take: Option<i64>, budget: Option<i64>) -> Self {
        let count = |value: i64| u64::try_from(value).unwrap_or(0);
        Self {
            skip: count(skip),
            take: take.map(count),
            budget: budget.map(count),
            position: 0,
            batch: 0,
            collected: 0,
            exhausted: false,
            looked_ahead: false,
            overflow_after_skip: None,
            matched_after_skip: 0,
        }
    }

    /// Apply an unlimited read's fail-closed `max_limit` to the matching rows
    /// after the skipped visible rows, counted before row policy.
    pub(crate) fn with_overflow_after_skip(mut self, maximum: Option<i64>) -> Self {
        self.overflow_after_skip = maximum.map(|maximum| u64::try_from(maximum).unwrap_or(0));
        self
    }

    fn complete(&self) -> bool {
        // An empty page needs no rows, whatever the offset. An unlimited read
        // completes only when the rows run out.
        self.take == Some(0)
            || (self.skip == 0 && self.take.is_some_and(|take| self.collected >= take))
    }

    /// Whether the scan still needs rows.
    pub(crate) fn wants_more(&self) -> bool {
        !self.exhausted && !self.complete()
    }

    /// The next batch to read, or `None` when the page is complete or the
    /// matching rows are exhausted.
    ///
    /// # Errors
    ///
    /// `PAGE_LIMIT_EXCEEDED` when the scan budget is spent and more rows remain.
    pub(crate) fn next_batch(&mut self) -> crate::Result<Option<ScanBatch>> {
        if !self.wants_more() {
            return Ok(None);
        }
        let needed = self.skip.saturating_add(match self.take {
            Some(take) => take - self.collected,
            // An unlimited read first asks for its whole fail-closed window.
            None => self
                .overflow_after_skip
                .map_or(Self::MAX_GROWN_BATCH, |maximum| maximum.saturating_add(1)),
        });
        let wanted = needed.max(self.batch).max(1);
        self.batch = wanted.saturating_mul(2).min(Self::MAX_GROWN_BATCH);
        let (examine, fetch) = match self.budget {
            // A spent budget still reads one row to learn whether any remain,
            // so an empty result is not refused.
            Some(0) if self.looked_ahead => return Err(scan_budget_exhausted()),
            Some(remaining) if wanted >= remaining => {
                self.looked_ahead = true;
                (remaining, remaining.saturating_add(1))
            }
            _ => (wanted, wanted),
        };
        let as_i64 = |value: u64| i64::try_from(value).unwrap_or(i64::MAX);
        Ok(Some(ScanBatch {
            offset: as_i64(self.position),
            fetch: as_i64(fetch),
            examine: usize::try_from(examine).unwrap_or(usize::MAX),
        }))
    }

    /// Count one examined candidate against an unlimited read's fail-closed
    /// `max_limit`, before row policy decides whether it is visible.
    ///
    /// # Errors
    ///
    /// `PAGE_LIMIT_EXCEEDED`, naming no row count, when more matching rows
    /// than the maximum follow the skipped rows.
    pub(crate) fn count_candidate(&mut self, matches: bool) -> crate::Result<()> {
        if let Some(maximum) = self.overflow_after_skip
            && matches
            && self.skip == 0
        {
            self.matched_after_skip += 1;
            let maximum = i64::try_from(maximum).unwrap_or(i64::MAX);
            RepositoryBound::FailClosed(maximum)
                .check_overflow(usize::try_from(self.matched_after_skip).unwrap_or(usize::MAX))?;
        }
        Ok(())
    }

    /// Record one examined row. Returns whether the caller keeps it.
    pub(crate) fn record(&mut self, visible: bool) -> bool {
        self.position += 1;
        if !visible {
            return false;
        }
        if self.skip > 0 {
            self.skip -= 1;
            return false;
        }
        self.collected += 1;
        true
    }

    /// Account for a finished batch that returned `fetched` rows, of which
    /// the caller examined `examined`.
    ///
    /// The rows ran out only when the batch came back short and every fetched
    /// row was examined. A scan that completed part-way through a batch has
    /// not run out of rows, so the rows after it are not lost.
    pub(crate) fn finish_batch(&mut self, batch: ScanBatch, fetched: usize, examined: usize) {
        if let Some(budget) = &mut self.budget {
            let read = fetched.min(batch.examine);
            *budget = budget.saturating_sub(u64::try_from(read).unwrap_or(u64::MAX));
        }
        if i64::try_from(fetched).unwrap_or(i64::MAX) < batch.fetch && examined == fetched {
            self.exhausted = true;
        }
    }

    /// Offset, in candidate rows, of the first row after the rows examined so
    /// far.
    #[cfg(test)]
    pub(crate) fn position(&self) -> i64 {
        i64::try_from(self.position).unwrap_or(i64::MAX)
    }
}

pub(crate) fn scan_budget_exhausted() -> sqlx::Error {
    use crate::graphql::errors::{OrmErrorCode, OrmPublicError, sqlx_error_from_public};
    sqlx_error_from_public(OrmPublicError::with_message(
        OrmErrorCode::PageLimitExceeded,
        "the row scan budget ran out before the page was complete; raise it with with_max_scanned_rows",
    ))
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
    /// Restrict delivered actions; an empty list matches no events.
    pub actions: Option<Vec<ChangeAction>>,
    /// Unsupported compatibility placeholder. Supplying any value is rejected
    /// with INVALID_INPUT at generated subscription admission.
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
/// a limit of [`PaginationConfig::DEFAULT_LIMIT`] (`50`) when this input or its
/// `limit` field is omitted, and clamps explicit limits to
/// [`PaginationConfig::DEFAULT_MAX_LIMIT`] (`100`). Use
/// [`PaginationConfig::legacy`] on `Database` for the previous 1000/1000 caps
/// during migration.
pub struct PageInput {
    /// Requested page size. Explicit limits are clamped by the runtime
    /// [`PaginationConfig`] before SQL rendering.
    pub limit: Option<i64>,
    /// Requested offset. Negative offsets are treated as `0`.
    pub offset: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeysetDirection {
    Asc,
    Desc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeysetNulls {
    First,
    Last,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeysetOrderColumn {
    pub column: &'static str,
    pub direction: KeysetDirection,
    pub nulls: KeysetNulls,
}

impl KeysetOrderColumn {
    pub const fn asc(column: &'static str) -> Self {
        Self {
            column,
            direction: KeysetDirection::Asc,
            nulls: KeysetNulls::Last,
        }
    }

    pub const fn desc(column: &'static str) -> Self {
        Self {
            column,
            direction: KeysetDirection::Desc,
            nulls: KeysetNulls::Last,
        }
    }

    pub const fn nulls_first(mut self) -> Self {
        self.nulls = KeysetNulls::First;
        self
    }

    /// Reverses direction and null placement for a backward database read.
    pub const fn reversed(mut self) -> Self {
        self.direction = match self.direction {
            KeysetDirection::Asc => KeysetDirection::Desc,
            KeysetDirection::Desc => KeysetDirection::Asc,
        };
        self.nulls = match self.nulls {
            KeysetNulls::First => KeysetNulls::Last,
            KeysetNulls::Last => KeysetNulls::First,
        };
        self
    }

    pub fn order_sql(self) -> String {
        let direction = match self.direction {
            KeysetDirection::Asc => "ASC",
            KeysetDirection::Desc => "DESC",
        };
        let null_rank = match self.nulls {
            KeysetNulls::First => 0,
            KeysetNulls::Last => 1,
        };
        let nonnull_rank = 1 - null_rank;
        format!(
            "CASE WHEN {} IS NULL THEN {} ELSE {} END ASC, {} {}",
            self.column, null_rank, nonnull_rank, self.column, direction
        )
    }
}

pub(crate) fn keyset_sql_value(
    value: &crate::graphql::pagination::KeysetValue,
) -> Option<SqlValue> {
    use crate::graphql::pagination::KeysetValue;
    match value {
        KeysetValue::Null => None,
        KeysetValue::String(value) => Some(SqlValue::String(value.clone())),
        KeysetValue::Int(value) => Some(SqlValue::Int(*value)),
        KeysetValue::Float(value) => Some(SqlValue::Float(*value)),
        KeysetValue::Bool(value) => Some(SqlValue::Bool(*value)),
        KeysetValue::Uuid(value) => uuid::Uuid::parse_str(value).ok().map(SqlValue::Uuid),
        KeysetValue::Bytes(value) => Some(SqlValue::Bytes(value.clone())),
    }
}

/// Render the lexicographic `after` predicate with explicit nullable ordering.
pub fn render_keyset_after(
    backend: DatabaseBackend,
    columns: &[KeysetOrderColumn],
    cursor: &[crate::graphql::pagination::KeysetValue],
    start_index: usize,
) -> Result<(String, Vec<SqlValue>), crate::graphql::errors::OrmPublicError> {
    use crate::graphql::errors::{OrmErrorCode, OrmPublicError};
    use crate::graphql::pagination::KeysetValue;
    if columns.len() != cursor.len() || columns.is_empty() {
        return Err(OrmPublicError::new(OrmErrorCode::CursorInvalid));
    }
    let mut branches = Vec::new();
    let mut values = Vec::new();
    for index in 0..columns.len() {
        let mut terms = Vec::new();
        for prior in 0..index {
            let column = columns[prior];
            match &cursor[prior] {
                KeysetValue::Null => terms.push(format!("{} IS NULL", column.column)),
                value => {
                    let value = keyset_sql_value(value)
                        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::CursorInvalid))?;
                    terms.push(format!(
                        "{} = {}",
                        column.column,
                        backend.placeholder(start_index + values.len())
                    ));
                    values.push(value);
                }
            }
        }
        let column = columns[index];
        let comparison = match &cursor[index] {
            KeysetValue::Null => match column.nulls {
                KeysetNulls::First => format!("{} IS NOT NULL", column.column),
                KeysetNulls::Last => "1 = 0".to_string(),
            },
            value => {
                let value = keyset_sql_value(value)
                    .ok_or_else(|| OrmPublicError::new(OrmErrorCode::CursorInvalid))?;
                let operator = match column.direction {
                    KeysetDirection::Asc => ">",
                    KeysetDirection::Desc => "<",
                };
                let scalar = format!(
                    "{} {} {}",
                    column.column,
                    operator,
                    backend.placeholder(start_index + values.len())
                );
                values.push(value);
                match column.nulls {
                    KeysetNulls::First => format!("({} IS NOT NULL AND {scalar})", column.column),
                    KeysetNulls::Last => format!("({} IS NULL OR {scalar})", column.column),
                }
            }
        };
        terms.push(comparison);
        branches.push(format!("({})", terms.join(" AND ")));
    }
    Ok((branches.join(" OR "), values))
}

/// Render the lexicographic `before` predicate with explicit nullable ordering.
pub fn render_keyset_before(
    backend: DatabaseBackend,
    columns: &[KeysetOrderColumn],
    cursor: &[crate::graphql::pagination::KeysetValue],
    start_index: usize,
) -> Result<(String, Vec<SqlValue>), crate::graphql::errors::OrmPublicError> {
    use crate::graphql::errors::{OrmErrorCode, OrmPublicError};
    use crate::graphql::pagination::KeysetValue;
    if columns.len() != cursor.len() || columns.is_empty() {
        return Err(OrmPublicError::new(OrmErrorCode::CursorInvalid));
    }
    let mut branches = Vec::new();
    let mut values = Vec::new();
    for index in 0..columns.len() {
        let mut terms = Vec::new();
        for prior in 0..index {
            let column = columns[prior];
            match &cursor[prior] {
                KeysetValue::Null => terms.push(format!("{} IS NULL", column.column)),
                value => {
                    let value = keyset_sql_value(value)
                        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::CursorInvalid))?;
                    terms.push(format!(
                        "{} = {}",
                        column.column,
                        backend.placeholder(start_index + values.len())
                    ));
                    values.push(value);
                }
            }
        }
        let column = columns[index];
        let comparison = match &cursor[index] {
            KeysetValue::Null => match column.nulls {
                KeysetNulls::First => "1 = 0".to_string(),
                KeysetNulls::Last => format!("{} IS NOT NULL", column.column),
            },
            value => {
                let value = keyset_sql_value(value)
                    .ok_or_else(|| OrmPublicError::new(OrmErrorCode::CursorInvalid))?;
                let operator = match column.direction {
                    KeysetDirection::Asc => "<",
                    KeysetDirection::Desc => ">",
                };
                let scalar = format!(
                    "{} {} {}",
                    column.column,
                    operator,
                    backend.placeholder(start_index + values.len())
                );
                values.push(value);
                match column.nulls {
                    KeysetNulls::First => {
                        format!("({} IS NULL OR {scalar})", column.column)
                    }
                    KeysetNulls::Last => {
                        format!("({} IS NOT NULL AND {scalar})", column.column)
                    }
                }
            }
        };
        terms.push(comparison);
        branches.push(format!("({})", terms.join(" AND ")));
    }

    Ok((format!("({})", branches.join(" OR ")), values))
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

    /// Observe a completed read without disclosing parameter values.
    fn observe_read(&self, _sql: &str, _rows: usize) {}
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
    fn observe_read(&self, sql: &str, rows: usize) {
        crate::db::Database::observe_read(self, sql, rows);
    }

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
    ) -> crate::Result<()>;

    async fn bulk_load_relations(
        entities: &mut [Self],
        pool: &B::Pool,
        selection: &[async_graphql::context::SelectionField<'_>],
    ) -> crate::Result<()>
    where
        Self: Sized;

    async fn load_relations_with_auth(
        &mut self,
        pool: &B::Pool,
        selection: &[async_graphql::context::SelectionField<'_>],
        _auth: Option<&DbAuthContext>,
    ) -> crate::Result<()> {
        self.load_relations(pool, selection).await
    }

    async fn bulk_load_relations_with_auth(
        entities: &mut [Self],
        pool: &B::Pool,
        selection: &[async_graphql::context::SelectionField<'_>],
        _auth: Option<&DbAuthContext>,
    ) -> crate::Result<()>
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
        FilterExpression::Raw { clause, values }
        | FilterExpression::TrustedFragment { clause, values } => {
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
        FilterExpression::Or(filters) => {
            if filters.is_empty() {
                return "1 = 0".to_owned();
            }
            filters
                .iter()
                .map(|filter| {
                    let sql = render_filter_expression(dialect, filter, next_index, bind_values);
                    if sql.is_empty() {
                        "(1 = 1)".to_owned()
                    } else {
                        format!("({sql})")
                    }
                })
                .collect::<Vec<_>>()
                .join(" OR ")
        }
    }
}

fn render_select_query_inner(
    dialect: DatabaseBackend,
    query: &SelectQuery,
    write_decision: bool,
) -> RenderedQuery {
    let projection = if query.count_only {
        dialect.count_projection().to_string()
    } else {
        query.columns.join(", ")
    };
    let table = dialect.render_identifier_path(query.table);
    let table = if write_decision && dialect == DatabaseBackend::Mssql {
        format!("{table} WITH (UPDLOCK, HOLDLOCK)")
    } else {
        table.into_owned()
    };
    let mut sql = format!("SELECT {} FROM {}", projection, table);
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
                .map(|sort| {
                    let rendered = dialect.normalize_sql(&sort.clause, next_index);
                    next_index += sort.values.len();
                    values.extend(sort.values.iter().cloned());
                    rendered
                })
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

    if write_decision && dialect == DatabaseBackend::Postgres {
        sql.push_str(" FOR UPDATE");
    }

    RenderedQuery { sql, values }
}

pub fn render_select_query(dialect: DatabaseBackend, query: &SelectQuery) -> RenderedQuery {
    render_select_query_inner(dialect, query, false)
}

fn render_write_decision_select_query(
    dialect: DatabaseBackend,
    query: &SelectQuery,
) -> RenderedQuery {
    render_select_query_inner(dialect, query, true)
}

pub fn render_aggregate_query(dialect: DatabaseBackend, query: &AggregateQuery) -> RenderedQuery {
    let argument = query.column.as_deref().unwrap_or("*");
    let projection = format!(
        "{}({}) AS __gom_aggregate",
        query.function.as_sql(),
        argument
    );
    let mut sql = format!(
        "SELECT {} FROM {}",
        projection,
        dialect.render_identifier_path(query.table)
    );
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

fn aggregate_group_projection(dialect: DatabaseBackend, column: &str, alias: &str) -> String {
    match dialect {
        DatabaseBackend::Mssql => format!("CAST({column} AS NVARCHAR(4000)) AS {alias}"),
        _ => format!("CAST({column} AS TEXT) AS {alias}"),
    }
}

fn aggregate_metric_projection(
    dialect: DatabaseBackend,
    expression: &AggregateExpression,
    alias: &str,
) -> String {
    use super::GraphqlAggregateOperator as Operator;
    let column = expression
        .field
        .map(|field| dialect.quote_identifier_path(field.column_name));
    let argument = column.as_deref().unwrap_or("*");
    let kind = expression.field.map(|field| field.kind);
    match (expression.operator, kind, dialect) {
        (Operator::Count, _, DatabaseBackend::Mssql) => {
            format!("COUNT_BIG({argument}) AS {alias}")
        }
        (Operator::Count, _, _) => format!("COUNT({argument}) AS {alias}"),
        (Operator::Sum, Some(AggregateFieldKind::Integral), DatabaseBackend::Postgres) => {
            format!("CAST(SUM(CAST({argument} AS NUMERIC(38,0))) AS TEXT) AS {alias}")
        }
        (Operator::Sum, Some(AggregateFieldKind::Integral), DatabaseBackend::Mssql) => {
            format!("CAST(SUM(CAST({argument} AS DECIMAL(38,0))) AS NVARCHAR(64)) AS {alias}")
        }
        (Operator::Sum, Some(AggregateFieldKind::Decimal(_)), DatabaseBackend::Postgres) => {
            format!("CAST(SUM({argument}) AS TEXT) AS {alias}")
        }
        (Operator::Sum, Some(AggregateFieldKind::Decimal(_)), DatabaseBackend::Mssql) => {
            format!("CAST(SUM({argument}) AS NVARCHAR(64)) AS {alias}")
        }
        (operator, Some(AggregateFieldKind::Decimal(_)), DatabaseBackend::Postgres) => {
            format!(
                "CAST({}({argument}) AS TEXT) AS {alias}",
                aggregate_operator_sql(operator)
            )
        }
        (operator, Some(AggregateFieldKind::Decimal(_)), DatabaseBackend::Mssql) => format!(
            "CAST({}({argument}) AS NVARCHAR(64)) AS {alias}",
            aggregate_operator_sql(operator)
        ),
        (operator, Some(AggregateFieldKind::Text), DatabaseBackend::Postgres) => format!(
            "CAST({}({argument}) AS TEXT) AS {alias}",
            aggregate_operator_sql(operator)
        ),
        (operator, Some(AggregateFieldKind::Text), DatabaseBackend::Mssql) => format!(
            "CAST({}({argument}) AS NVARCHAR(4000)) AS {alias}",
            aggregate_operator_sql(operator)
        ),
        (operator, _, _) => {
            format!(
                "{}({argument}) AS {alias}",
                aggregate_operator_sql(operator)
            )
        }
    }
}

fn aggregate_operator_sql(operator: super::GraphqlAggregateOperator) -> &'static str {
    #[allow(unreachable_patterns)]
    match operator {
        super::GraphqlAggregateOperator::Count => "COUNT",
        super::GraphqlAggregateOperator::Min => "MIN",
        super::GraphqlAggregateOperator::Max => "MAX",
        super::GraphqlAggregateOperator::Sum => "SUM",
        _ => unreachable!("unsupported aggregate operator from a newer catalogue"),
    }
}

/// Renders one validated multi-expression grouped aggregate query.
fn render_grouped_aggregate_query(
    dialect: DatabaseBackend,
    query: &GroupedAggregateSqlQuery,
) -> RenderedQuery {
    render_grouped_aggregate_query_page(dialect, query, None)
}

fn render_grouped_aggregate_query_page(
    dialect: DatabaseBackend,
    query: &GroupedAggregateSqlQuery,
    page: Option<(AggregateGroupOrder, Option<&AggregateGroupCursor>)>,
) -> RenderedQuery {
    let mut quoted_groups = query
        .groups
        .iter()
        .map(|field| dialect.quote_identifier_path(field.column_name))
        .collect::<Vec<_>>();
    // PostgreSQL page identities are exact text, independent of the source's
    // locale/nondeterministic collation. Ordinary aggregates retain native equality.
    if page.is_some() && dialect == DatabaseBackend::Postgres {
        quoted_groups = quoted_groups
            .iter()
            .map(|column| format!("CAST({column} AS TEXT) COLLATE pg_catalog.\"C\""))
            .collect();
    }
    // Preserve native SQLite GROUP BY equality. A deterministic original representative
    // makes continuation stable even if native equality folds text variants.
    let ordered_groups = if page.is_some() {
        quoted_groups
            .iter()
            .map(|column| match dialect {
                DatabaseBackend::Postgres => column.clone(),
                _ => format!("MIN({column} COLLATE BINARY)"),
            })
            .collect::<Vec<_>>()
    } else {
        quoted_groups.clone()
    };
    let mut projections = ordered_groups
        .iter()
        .enumerate()
        .map(|(index, column)| {
            aggregate_group_projection(dialect, column, &format!("__gom_group_{index:03}"))
        })
        .collect::<Vec<_>>();
    projections.extend(query.metrics.iter().enumerate().map(|(index, expression)| {
        aggregate_metric_projection(dialect, expression, &format!("__gom_metric_{index:03}"))
    }));

    let mut sql = format!(
        "SELECT {} FROM {}",
        projections.join(", "),
        dialect.render_identifier_path(query.table)
    );
    let mut values = Vec::new();
    let mut next_index = 1usize;
    if let Some(filter) = &query.filter {
        let where_sql = render_filter_expression(dialect, filter, &mut next_index, &mut values);
        if !where_sql.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&where_sql);
        }
    }
    if !quoted_groups.is_empty() {
        sql.push_str(" GROUP BY ");
        sql.push_str(&quoted_groups.join(", "));
        if let Some((order, Some(cursor))) = page {
            let column = &ordered_groups[0];
            let boundary = match &cursor.key {
                None => FilterExpression::trusted_fragment(format!("{column} IS NOT NULL"), vec![]),
                Some(key) => {
                    let binary = group_binary_comparison(dialect, column);
                    let (clause, values) = match order {
                        AggregateGroupOrder::Binary => {
                            (format!("{binary} > ?"), vec![SqlValue::String(key.clone())])
                        }
                        AggregateGroupOrder::SqliteNoCase => {
                            let folded = group_ascii_comparison(dialect, column);
                            let bound = group_ascii_comparison(dialect, "?");
                            (
                                format!(
                                    "({folded} > {bound} OR ({folded} = {bound} AND {binary} > ?))"
                                ),
                                vec![SqlValue::String(key.clone()); 3],
                            )
                        }
                    };
                    FilterExpression::trusted_fragment(clause, values)
                }
            };
            sql.push_str(" HAVING ");
            sql.push_str(&render_filter_expression(
                dialect,
                &boundary,
                &mut next_index,
                &mut values,
            ));
        }
        sql.push_str(" ORDER BY ");
        sql.push_str(
            &ordered_groups
                .iter()
                .flat_map(|column| {
                    let mut order =
                        vec![format!("CASE WHEN {column} IS NULL THEN 0 ELSE 1 END ASC")];
                    if let Some((mode, _)) = page {
                        if mode == AggregateGroupOrder::SqliteNoCase {
                            order.push(format!("{} ASC", group_ascii_comparison(dialect, column)));
                        }
                        order.push(format!("{} ASC", group_binary_comparison(dialect, column)));
                    } else {
                        order.push(format!("{column} ASC"));
                    }
                    order
                })
                .collect::<Vec<_>>()
                .join(", "),
        );
        sql.push_str(&dialect.render_pagination(Some(query.group_limit), 0));
    }
    RenderedQuery { sql, values }
}

fn group_binary_comparison(dialect: DatabaseBackend, value: &str) -> String {
    match dialect {
        DatabaseBackend::Postgres => format!("({value}) COLLATE pg_catalog.\"C\""),
        _ => format!("{value} COLLATE BINARY"),
    }
}

fn group_ascii_comparison(dialect: DatabaseBackend, value: &str) -> String {
    match dialect {
        DatabaseBackend::Postgres => format!(
            "pg_catalog.translate(({value})::pg_catalog.text, 'ABCDEFGHIJKLMNOPQRSTUVWXYZ', 'abcdefghijklmnopqrstuvwxyz') COLLATE pg_catalog.\"C\""
        ),
        _ => format!("{value} COLLATE NOCASE"),
    }
}

/// Ascending text-group comparison on supported page providers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AggregateGroupOrder {
    /// Byte comparison (SQLite BINARY / PostgreSQL C), with SQL NULL first.
    #[default]
    Binary,
    /// ASCII-only folding followed by byte comparison for deterministic ties.
    /// The historical name is retained: PostgreSQL uses explicit ASCII translation
    /// and C comparison, equivalent to SQLite NOCASE on UTF-8 text.
    SqliteNoCase,
}

/// Request-local settings for complete bounded string-group enumeration.
#[derive(Clone, Debug)]
pub struct AggregateGroupPageOptions {
    /// Comparison applied to grouping representatives; not to group equality.
    pub order: AggregateGroupOrder,
    /// Apply SQL TRIM(key) <> '' before grouping. Original nonblank keys are retained.
    pub exclude_blank: bool,
    /// Trusted host authorization partition/public revision, never an authority grant.
    pub context: String,
}

/// Framework-neutral typed boundary. Hosts may wrap/protect its bounded JSON.
/// No existing static or runtime cursor format is changed.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct AggregateGroupCursor {
    format_version: u8,
    fingerprint: String,
    key: Option<String>,
}
impl AggregateGroupCursor {
    /// Parse bounded JSON with strict shape checking before allocating an unbounded cursor.
    pub fn from_json(text: &str) -> crate::Result<Self> {
        if text.len() > 256 * 1024 {
            return Err(invalid_aggregate("invalid aggregate group cursor"));
        }
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            format_version: u8,
            fingerprint: String,
            key: serde_json::Value,
        }
        let wire: Wire = serde_json::from_str(text)
            .map_err(|_| invalid_aggregate("invalid aggregate group cursor"))?;
        let cursor = Self {
            format_version: wire.format_version,
            fingerprint: wire.fingerprint,
            key: match wire.key {
                serde_json::Value::Null => None,
                serde_json::Value::String(key) => Some(key),
                _ => return Err(invalid_aggregate("invalid aggregate group cursor")),
            },
        };
        cursor.validate()?;
        Ok(cursor)
    }
    /// Serialize the complete boundary for a host-owned cursor envelope.
    pub fn to_json(&self) -> crate::Result<String> {
        self.validate()?;
        let text = serde_json::to_string(self)
            .map_err(|_| invalid_aggregate("invalid aggregate group cursor"))?;
        if text.len() > 256 * 1024 {
            return Err(invalid_aggregate("invalid aggregate group cursor"));
        }
        Ok(text)
    }
    /// Exact original grouping representative; None means SQL NULL.
    pub fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }
    fn validate(&self) -> crate::Result<()> {
        if self.format_version != 1
            || self.fingerprint.len() != 64
            || !self
                .fingerprint
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            || self.key.as_ref().is_some_and(|key| key.len() > 65536)
        {
            return Err(invalid_aggregate("invalid aggregate group cursor"));
        }
        Ok(())
    }
}

/// At most group_limit result groups, with a database lookahead and typed continuation.
#[derive(Clone, Debug)]
pub struct AggregateGroupPage {
    /// Ordered groups with exact original representatives and optional metric values.
    pub rows: Vec<AggregateResultRow>,
    /// True only when the database returned one additional authorized group.
    pub has_next_page: bool,
    /// Last returned boundary, including on the final nonempty page.
    pub end_cursor: Option<AggregateGroupCursor>,
}

/// Policy-aware typed multi-expression aggregate builder.
pub struct GroupedAggregateQuery<'a, T, F, A, B: OrmBackend = DefaultBackend>
where
    T: Entity,
    F: DatabaseFilter,
    A: TypedAggregateField<T>,
{
    db: &'a crate::db::Database<B>,
    auth: Option<DbAuthContext>,
    filter: Option<F>,
    groups: Vec<AggregateFieldRef>,
    metrics: Vec<AggregateExpression>,
    group_limit: i64,
    _entity: PhantomData<(T, A)>,
}

impl<'a, T, F, A, B> GroupedAggregateQuery<'a, T, F, A, B>
where
    B: OrmBackend,
    T: Entity,
    F: DatabaseFilter,
    A: TypedAggregateField<T>,
{
    /// Creates an aggregate builder with the database's bounded default group limit.
    pub fn new(db: &'a crate::db::Database<B>) -> Self {
        let group_limit = db
            .pagination_config()
            .resolve_page(None, true)
            .limit
            .unwrap_or(PaginationConfig::SECURE_DEFAULT_LIMIT);
        Self {
            db,
            auth: None,
            filter: None,
            groups: Vec::new(),
            metrics: Vec::new(),
            group_limit,
            _entity: PhantomData,
        }
    }

    /// Applies backend transaction-local authorization such as PostgreSQL RLS context.
    pub fn with_auth(mut self, auth: Option<&DbAuthContext>) -> Self {
        self.auth = auth.cloned();
        self
    }

    /// Applies one generated typed filter.
    pub fn filter(mut self, filter: F) -> Self {
        self.filter = Some(filter);
        self
    }

    /// Adds one grouping field in deterministic declaration order.
    pub fn group_by(mut self, field: A) -> crate::Result<Self> {
        let field = *field.aggregate_field();
        if !field.groupable || self.groups.contains(&field) || self.groups.len() >= 16 {
            return Err(invalid_aggregate(
                "invalid or duplicate aggregate grouping field",
            ));
        }
        self.groups.push(field);
        Ok(self)
    }

    /// Adds a row-count metric.
    pub fn count_rows(self) -> crate::Result<Self> {
        self.push_metric(super::GraphqlAggregateOperator::Count, None)
    }

    /// Adds a non-null-value count metric.
    pub fn count(self, field: A) -> crate::Result<Self> {
        self.push_metric(
            super::GraphqlAggregateOperator::Count,
            Some(*field.aggregate_field()),
        )
    }

    /// Adds a minimum metric.
    pub fn min(self, field: A) -> crate::Result<Self> {
        self.push_metric(
            super::GraphqlAggregateOperator::Min,
            Some(*field.aggregate_field()),
        )
    }

    /// Adds a maximum metric.
    pub fn max(self, field: A) -> crate::Result<Self> {
        self.push_metric(
            super::GraphqlAggregateOperator::Max,
            Some(*field.aggregate_field()),
        )
    }

    /// Adds an exact sum metric.
    pub fn sum(self, field: A) -> crate::Result<Self> {
        self.push_metric(
            super::GraphqlAggregateOperator::Sum,
            Some(*field.aggregate_field()),
        )
    }

    fn push_metric(
        mut self,
        operator: super::GraphqlAggregateOperator,
        field: Option<AggregateFieldRef>,
    ) -> crate::Result<Self> {
        if self.metrics.len() >= 32 {
            return Err(invalid_aggregate("too many aggregate expressions"));
        }
        if let Some(field) = field
            && !field.operators.contains(&operator)
        {
            return Err(invalid_aggregate(
                "aggregate operator is not valid for field",
            ));
        }
        if self
            .metrics
            .iter()
            .any(|metric| metric.operator == operator && metric.field == field)
        {
            return Err(invalid_aggregate("duplicate aggregate expression"));
        }
        self.metrics.push(AggregateExpression { operator, field });
        Ok(self)
    }

    /// Sets a positive result-group bound that may not exceed the database maximum.
    pub fn group_limit(mut self, limit: i64) -> crate::Result<Self> {
        if limit <= 0
            || self
                .db
                .pagination_config()
                .max_limit
                .is_some_and(|maximum| limit > maximum)
        {
            return Err(invalid_aggregate(
                "aggregate group limit is outside configured bounds",
            ));
        }
        self.group_limit = limit;
        Ok(self)
    }

    async fn authorize(
        &self,
        graphql_context: Option<&async_graphql::Context<'_>>,
    ) -> crate::Result<()> {
        let metadata = T::metadata();
        let surface = if graphql_context.is_some() {
            super::core::EntityAccessSurface::GraphqlQuery
        } else {
            super::core::EntityAccessSurface::Repository
        };
        self.db
            .ensure_entity_access(
                graphql_context,
                metadata.entity_name,
                metadata.read_policy,
                super::core::EntityAccessKind::Read,
                surface,
            )
            .await
            .map_err(crate::graphql::errors::sqlx_error_from_graphql)?;
        let mut fields = self
            .groups
            .iter()
            .chain(
                self.metrics
                    .iter()
                    .filter_map(|metric| metric.field.as_ref()),
            )
            .map(|field| field.api_name)
            .collect::<Vec<_>>();
        if let Some(filter) = &self.filter {
            if filter.requires_in_memory_filtering(B::DIALECT) {
                return Err(invalid_aggregate(
                    "aggregate filters must be rendered completely by the database",
                ));
            }
            let referenced = filter.referenced_fields();
            if !filter.is_empty()
                && referenced.is_empty()
                && !filter.has_request_authorized_predicates()
            {
                return Err(invalid_aggregate(
                    "aggregate filter did not provide field authorization evidence",
                ));
            }
            fields.extend(referenced);
        }
        fields.sort_unstable();
        fields.dedup();
        for field_name in fields {
            let field = T::repository_field_policies()
                .iter()
                .find(|field| field.api_name == field_name)
                .ok_or_else(|| invalid_aggregate("aggregate field policy metadata is missing"))?;
            if let Some(context) = graphql_context {
                self.db
                    .ensure_readable_field(
                        context,
                        metadata.entity_name,
                        field.api_name,
                        field.read_policy,
                        None,
                    )
                    .await
                    .map_err(crate::graphql::errors::sqlx_error_from_graphql)?;
            } else {
                self.db
                    .ensure_repository_readable_field(
                        None,
                        metadata.entity_name,
                        field.api_name,
                        field.read_policy,
                        None,
                    )
                    .await
                    .map_err(crate::graphql::errors::sqlx_error_from_public)?;
            }
        }
        Ok(())
    }

    /// Enumerate all distinct string groups through bounded SQLite/PostgreSQL pages.
    ///
    /// Exactly one text group, ascending byte/ASCII-folded ordering, optional
    /// existing metrics and fully SQL-authorized rows. Unsupported backends,
    /// scalar groups and residual policies fail before the aggregate SELECT.
    /// The existing group_limit bounds each page (1..=1000 plus one lookahead).
    pub async fn fetch_group_page(
        self,
        options: AggregateGroupPageOptions,
        after: Option<&AggregateGroupCursor>,
    ) -> crate::Result<AggregateGroupPage>
    where
        T: 'static,
    {
        if !matches!(
            B::DIALECT,
            DatabaseBackend::Sqlite | DatabaseBackend::Postgres
        ) || self.groups.len() != 1
        {
            return Err(invalid_aggregate(
                "unsupported aggregate group pagination capability",
            ));
        }
        let field = self.groups[0];
        let text_storage = T::columns()
            .iter()
            .find(|c| c.name == field.column_name)
            .is_some_and(|c| {
                let ty = c.sql_type.to_ascii_uppercase();
                if B::DIALECT == DatabaseBackend::Postgres {
                    ty == "TEXT" || ty.starts_with("VARCHAR") || ty.starts_with("CHARACTER VARYING")
                } else {
                    ty.contains("TEXT") || ty.contains("CHAR") || ty.contains("CLOB")
                }
            });
        if field.kind != AggregateFieldKind::Text || !text_storage {
            return Err(invalid_aggregate(
                "aggregate group pages require text storage",
            ));
        }
        if !(1..=1000).contains(&self.group_limit)
            || self
                .db
                .pagination_config()
                .max_limit
                .is_some_and(|max| self.group_limit > max)
            || options.context.is_empty()
            || options.context.len() > 4096
        {
            return Err(invalid_aggregate(
                "invalid aggregate group page bounds or context",
            ));
        }
        if let Some(cursor) = after {
            cursor.validate()?;
            if cursor.key.is_none() && !field.nullable {
                return Err(invalid_aggregate("invalid aggregate group cursor"));
            }
        }
        if let Some(filter) = &self.filter {
            filter.validate()?;
        }
        self.authorize(None).await?;
        let visibility = self
            .db
            .read_visibility::<T>(None, super::EntityAccessSurface::Repository)
            .await
            .map_err(crate::graphql::errors::sqlx_error_from_graphql)?;
        if visibility.requires_residual_checks() {
            return Err(residual_visibility_refused(
                "aggregate group pages require complete SQL row authorization",
            ));
        }
        let mut filters: Vec<FilterExpression> = self
            .filter
            .as_ref()
            .and_then(DatabaseFilter::to_filter_expression)
            .into_iter()
            .collect();
        if let Some(predicate) = visibility.predicate_expression() {
            filters.push(predicate.clone());
        }
        if options.exclude_blank {
            filters.push(FilterExpression::trusted_fragment(
                {
                    let trim = format!("TRIM({})", B::DIALECT.quote_identifier(field.column_name));
                    if B::DIALECT == DatabaseBackend::Postgres {
                        format!("{} <> ?", group_binary_comparison(B::DIALECT, &trim))
                    } else {
                        format!("{trim} <> ?")
                    }
                },
                vec![SqlValue::String(String::new())],
            ));
        }
        let query = GroupedAggregateSqlQuery {
            table: T::TABLE_NAME,
            groups: self.groups.clone(),
            metrics: self.metrics.clone(),
            filter: if filters.is_empty() {
                None
            } else {
                Some(FilterExpression::And(filters))
            },
            group_limit: self.group_limit + 1,
        };
        use sha2::Digest;
        let identity = format!(
            "aggregate-groups-v1:{:?}",
            (
                B::DIALECT,
                T::metadata().entity_name,
                query.table,
                &query.groups,
                &query.metrics,
                &query.filter,
                options.order,
                options.exclude_blank,
                &options.context,
                self.auth.as_ref().map(DbAuthContext::canonical_key)
            )
        );
        let fingerprint = format!("{:x}", sha2::Sha256::digest(identity.as_bytes()));
        if after.is_some_and(|cursor| cursor.fingerprint != fingerprint) {
            return Err(invalid_aggregate("invalid aggregate group cursor"));
        }
        let rendered =
            render_grouped_aggregate_query_page(B::DIALECT, &query, Some((options.order, after)));
        let fetched = B::fetch_rows_with_auth(
            self.db.pool(),
            &rendered.sql,
            &rendered.values,
            self.auth.as_ref(),
        )
        .await?;
        self.db.observe_read(&rendered.sql, fetched.len());
        let has_next_page = fetched.len() > self.group_limit as usize;
        let rows: Vec<AggregateResultRow> = fetched
            .iter()
            .take(self.group_limit as usize)
            .map(|row| decode_aggregate_row::<B>(row, &query.groups, &query.metrics))
            .collect::<crate::Result<_>>()?;
        let end_cursor = rows
            .last()
            .map(|row| {
                let key = match &row.groups[0].value {
                    AggregateValue::Null => None,
                    AggregateValue::Text(key) => Some(key.clone()),
                    _ => return Err(invalid_aggregate("invalid aggregate group value")),
                };
                let cursor = AggregateGroupCursor {
                    format_version: 1,
                    fingerprint,
                    key,
                };
                cursor.to_json()?;
                Ok(cursor)
            })
            .transpose()?;
        Ok(AggregateGroupPage {
            rows,
            has_next_page,
            end_cursor,
        })
    }

    /// Executes aggregation in the database and decodes bounded result groups.
    pub async fn fetch(self) -> crate::Result<Vec<AggregateResultRow>> {
        self.fetch_authorized(None).await
    }

    /// Executes aggregation for a generated GraphQL resolver using GraphQL field policy context.
    #[doc(hidden)]
    pub async fn fetch_with_graphql_context(
        self,
        context: &async_graphql::Context<'_>,
    ) -> crate::Result<Vec<AggregateResultRow>> {
        self.fetch_authorized(Some(context)).await
    }

    async fn fetch_authorized(
        self,
        context: Option<&async_graphql::Context<'_>>,
    ) -> crate::Result<Vec<AggregateResultRow>> {
        if self.metrics.is_empty() {
            return Err(invalid_aggregate(
                "at least one aggregate expression is required",
            ));
        }
        if let Some(filter) = &self.filter {
            filter.validate()?;
        }
        self.authorize(context).await?;
        let visibility = self
            .db
            .aggregate_read_visibility::<T, A>(context)
            .await
            .map_err(crate::graphql::errors::sqlx_error_from_graphql)?;
        if visibility.requires_residual_checks() {
            return Err(residual_visibility_refused(
                "aggregate queries require complete SQL row authorization",
            ));
        }
        let filters: Vec<_> = self
            .filter
            .as_ref()
            .and_then(DatabaseFilter::to_filter_expression)
            .into_iter()
            .chain(visibility.predicate_expression().cloned())
            .collect();
        let filter = if filters.is_empty() {
            None
        } else {
            Some(FilterExpression::And(filters))
        };
        let query = GroupedAggregateSqlQuery {
            table: T::TABLE_NAME,
            groups: self.groups.clone(),
            metrics: self.metrics.clone(),
            filter,
            group_limit: self.group_limit,
        };
        let rendered = render_grouped_aggregate_query(B::DIALECT, &query);
        let rows = B::fetch_rows_with_auth(
            self.db.pool(),
            &rendered.sql,
            &rendered.values,
            self.auth.as_ref(),
        )
        .await?;
        self.db.observe_read(&rendered.sql, rows.len());
        rows.iter()
            .map(|row| decode_aggregate_row::<B>(row, &query.groups, &query.metrics))
            .collect()
    }
}

fn invalid_aggregate(message: &'static str) -> sqlx::Error {
    sqlx::Error::Protocol(message.to_owned())
}

/// A read path that needs SQL-only row visibility got a residual decision.
pub(crate) fn residual_visibility_refused(message: &'static str) -> sqlx::Error {
    crate::graphql::errors::sqlx_error_from_public(
        crate::graphql::errors::OrmPublicError::new(
            crate::graphql::errors::OrmErrorCode::AuthorizationMisconfigured,
        )
        .with_internal(message),
    )
}

fn decode_group_value<B: OrmBackend>(
    row: &B::Row,
    alias: &str,
    field: AggregateFieldRef,
) -> crate::Result<AggregateValue> {
    let Some(value) = B::try_get_optional_string(row, alias)? else {
        return Ok(AggregateValue::Null);
    };
    match field.kind {
        AggregateFieldKind::Integral => value
            .parse::<i128>()
            .map(AggregateValue::Integral)
            .map_err(|_| invalid_aggregate("invalid integral aggregate group value")),
        AggregateFieldKind::Decimal(definition) if B::DIALECT == DatabaseBackend::Sqlite => value
            .parse::<i64>()
            .map(|value| AggregateValue::Decimal(definition.from_scaled_i64(value)))
            .map_err(|_| invalid_aggregate("invalid decimal aggregate group value")),
        AggregateFieldKind::Decimal(definition) => value
            .parse::<rust_decimal::Decimal>()
            .map_err(|_| invalid_aggregate("invalid decimal aggregate group value"))
            .and_then(|value| {
                definition
                    .normalize(value)
                    .map_err(|_| invalid_aggregate("invalid decimal aggregate group value"))
            })
            .map(AggregateValue::Decimal),
        AggregateFieldKind::Floating => value
            .parse::<f64>()
            .map(AggregateValue::Floating)
            .map_err(|_| invalid_aggregate("invalid floating aggregate group value")),
        AggregateFieldKind::Boolean => match value.as_str() {
            "1" | "true" | "TRUE" => Ok(AggregateValue::Boolean(true)),
            "0" | "false" | "FALSE" => Ok(AggregateValue::Boolean(false)),
            _ => Err(invalid_aggregate("invalid boolean aggregate group value")),
        },
        AggregateFieldKind::Text => Ok(AggregateValue::Text(value)),
    }
}

fn decode_metric_value<B: OrmBackend>(
    row: &B::Row,
    alias: &str,
    metric: AggregateExpression,
) -> crate::Result<AggregateValue> {
    use super::GraphqlAggregateOperator as Operator;
    if metric.operator == Operator::Count {
        return B::try_get_i64(row, alias).map(AggregateValue::Count);
    }
    let field = metric
        .field
        .ok_or_else(|| invalid_aggregate("aggregate field is missing"))?;
    match field.kind {
        AggregateFieldKind::Integral
            if metric.operator == Operator::Sum && B::DIALECT != DatabaseBackend::Sqlite =>
        {
            let value = B::try_get_optional_string(row, alias)?;
            value
                .map(|value| {
                    value
                        .parse::<i128>()
                        .map(AggregateValue::Integral)
                        .map_err(|_| invalid_aggregate("integral aggregate overflow"))
                })
                .transpose()
                .map(|value| value.unwrap_or(AggregateValue::Null))
        }
        AggregateFieldKind::Integral => B::try_get_optional_i64(row, alias).map(|value| {
            value
                .map(|value| AggregateValue::Integral(i128::from(value)))
                .unwrap_or(AggregateValue::Null)
        }),
        AggregateFieldKind::Decimal(definition) if B::DIALECT == DatabaseBackend::Sqlite => {
            B::try_get_optional_i64(row, alias).map(|value| {
                value
                    .map(|value| AggregateValue::Decimal(definition.from_scaled_i64(value)))
                    .unwrap_or(AggregateValue::Null)
            })
        }
        AggregateFieldKind::Decimal(definition) => {
            let value = B::try_get_optional_string(row, alias)?;
            value
                .map(|value| {
                    value
                        .parse::<rust_decimal::Decimal>()
                        .map_err(|_| invalid_aggregate("invalid decimal aggregate value"))
                        .and_then(|value| {
                            definition
                                .normalize(value)
                                .map_err(|_| invalid_aggregate("invalid decimal aggregate value"))
                        })
                        .map(AggregateValue::Decimal)
                })
                .transpose()
                .map(|value| value.unwrap_or(AggregateValue::Null))
        }
        AggregateFieldKind::Floating => B::try_get_optional_f64(row, alias).map(|value| {
            value
                .map(AggregateValue::Floating)
                .unwrap_or(AggregateValue::Null)
        }),
        AggregateFieldKind::Text => B::try_get_optional_string(row, alias).map(|value| {
            value
                .map(AggregateValue::Text)
                .unwrap_or(AggregateValue::Null)
        }),
        AggregateFieldKind::Boolean => Err(invalid_aggregate("boolean min/max is not portable")),
    }
}

fn decode_aggregate_row<B: OrmBackend>(
    row: &B::Row,
    groups: &[AggregateFieldRef],
    metrics: &[AggregateExpression],
) -> crate::Result<AggregateResultRow> {
    let groups = groups
        .iter()
        .enumerate()
        .map(|(index, field)| {
            Ok(AggregateResultValue {
                field: Some(field.api_name),
                operator: None,
                value: decode_group_value::<B>(row, &format!("__gom_group_{index:03}"), *field)?,
            })
        })
        .collect::<crate::Result<Vec<_>>>()?;
    let metrics = metrics
        .iter()
        .enumerate()
        .map(|(index, metric)| {
            Ok(AggregateResultValue {
                field: metric.field.map(|field| field.api_name),
                operator: Some(metric.operator),
                value: decode_metric_value::<B>(row, &format!("__gom_metric_{index:03}"), *metric)?,
            })
        })
        .collect::<crate::Result<Vec<_>>>()?;
    Ok(AggregateResultRow { groups, metrics })
}

pub fn render_delete_query(dialect: DatabaseBackend, query: &DeleteQuery) -> RenderedQuery {
    let mut sql = format!(
        "DELETE FROM {}",
        dialect.render_identifier_path(query.table)
    );
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
    render_upsert_sql_with_managed_version(
        dialect,
        table,
        insert_columns,
        insert_values,
        conflict_columns,
        update_columns,
        update_updated_at,
        None,
    )
}

/// Render an upsert whose update branch also advances a managed version column.
///
/// The insert branch writes the declared default. The update branch sets
/// `version = version + 1` and only applies while the stored version is below
/// `i64::MAX`.
#[allow(clippy::too_many_arguments)]
pub fn render_upsert_sql_with_managed_version(
    dialect: DatabaseBackend,
    table: &str,
    insert_columns: &[&str],
    insert_values: &[&str],
    conflict_columns: &[&str],
    update_columns: &[&str],
    update_updated_at: bool,
    managed_version: Option<&str>,
) -> String {
    let quoted_table = dialect.quote_identifier_path(table);
    let quoted_insert_columns = insert_columns
        .iter()
        .map(|column| dialect.quote_identifier_path(column))
        .collect::<Vec<_>>();
    let quoted_conflict_columns = conflict_columns
        .iter()
        .map(|column| dialect.quote_identifier_path(column))
        .collect::<Vec<_>>();
    if dialect == DatabaseBackend::Mssql {
        let mut assignments = update_columns
            .iter()
            .map(|column| {
                let quoted = dialect.quote_identifier_path(column);
                format!("target.{quoted} = source.{quoted}",)
            })
            .collect::<Vec<_>>();
        if update_updated_at {
            assignments.push(format!(
                "target.{} = {}",
                dialect.quote_identifier("updated_at"),
                dialect.current_epoch_expr()
            ));
        }
        let mut predicates = conflict_columns
            .iter()
            .map(|column| {
                let quoted = dialect.quote_identifier_path(column);
                format!("target.{quoted} = source.{quoted}",)
            })
            .collect::<Vec<_>>();
        if let Some(column) = managed_version {
            let quoted = dialect.quote_identifier_path(column);
            assignments.push(format!("target.{quoted} = target.{quoted} + 1"));
            predicates.push(format!("target.{quoted} < {}", i64::MAX));
        }
        let predicates = predicates.join(" AND ");
        let source_projection = insert_values
            .iter()
            .zip(&quoted_insert_columns)
            .map(|(value, column)| format!("{value} AS {column}"))
            .collect::<Vec<_>>()
            .join(", ");
        return format!(
            "SELECT {source_projection} INTO #graphql_orm_upsert_source; \
             UPDATE target WITH (UPDLOCK, HOLDLOCK) SET {} \
             FROM {quoted_table} AS target CROSS JOIN #graphql_orm_upsert_source AS source \
             WHERE {predicates}; \
             IF @@ROWCOUNT = 0 BEGIN \
             INSERT INTO {quoted_table} ({}) SELECT {} FROM #graphql_orm_upsert_source; END; \
             DROP TABLE #graphql_orm_upsert_source",
            assignments.join(", "),
            quoted_insert_columns.join(", "),
            quoted_insert_columns.join(", "),
        );
    }
    let mut set_clauses = update_columns
        .iter()
        .map(|column| {
            let quoted = dialect.quote_identifier_path(column);
            format!("{quoted} = EXCLUDED.{quoted}")
        })
        .collect::<Vec<_>>();
    if update_updated_at {
        set_clauses.push(format!(
            "{} = {}",
            dialect.quote_identifier("updated_at"),
            dialect.current_epoch_expr()
        ));
    }
    let mut version_guard = String::new();
    if let Some(column) = managed_version {
        let quoted = dialect.quote_identifier_path(column);
        set_clauses.push(format!("{quoted} = {quoted_table}.{quoted} + 1"));
        version_guard = format!(" WHERE {quoted_table}.{quoted} < {}", i64::MAX);
    }

    format!(
        "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) DO UPDATE SET {}{}",
        quoted_table,
        quoted_insert_columns.join(", "),
        insert_values.join(", "),
        quoted_conflict_columns.join(", "),
        set_clauses.join(", "),
        version_guard
    )
}

/// Render a bound insert that ignores one declared primary/unique conflict.
pub fn render_insert_if_absent_sql(
    dialect: DatabaseBackend,
    table: &str,
    insert_columns: &[&str],
    insert_values: &[&str],
    conflict_columns: &[&str],
) -> String {
    let table = dialect.quote_identifier_path(table);
    let columns = insert_columns
        .iter()
        .map(|column| dialect.quote_identifier_path(column))
        .collect::<Vec<_>>()
        .join(", ");
    let conflicts = conflict_columns
        .iter()
        .map(|column| dialect.quote_identifier_path(column))
        .collect::<Vec<_>>()
        .join(", ");
    if dialect == DatabaseBackend::Mssql {
        let predicates = conflict_columns
            .iter()
            .map(|column| {
                let quoted = dialect.quote_identifier_path(column);
                format!("target.{quoted} = source.{quoted}")
            })
            .collect::<Vec<_>>()
            .join(" AND ");
        let source_projection = insert_values.join(", ");
        let source_columns = insert_columns
            .iter()
            .map(|column| format!("source.{}", dialect.quote_identifier_path(column)))
            .collect::<Vec<_>>()
            .join(", ");
        return format!(
            "INSERT INTO {table} ({columns}) SELECT {source_columns} \
             FROM (VALUES ({source_projection})) AS source ({columns}) \
             WHERE NOT EXISTS (SELECT 1 FROM {table} AS target WITH (UPDLOCK, HOLDLOCK) \
             WHERE {predicates})"
        );
    }
    format!(
        "INSERT INTO {table} ({columns}) VALUES ({}) ON CONFLICT ({conflicts}) DO NOTHING",
        insert_values.join(", ")
    )
}

pub fn render_insert_sql(
    dialect: DatabaseBackend,
    table: &str,
    insert_columns: &[&str],
    insert_values: &[&str],
) -> String {
    let table = dialect.quote_identifier_path(table);
    let columns = insert_columns
        .iter()
        .map(|column| dialect.quote_identifier_path(column))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "INSERT INTO {table} ({columns}) VALUES ({})",
        insert_values.join(", ")
    )
}

pub fn build_insert_sql(table: &str, insert_columns: &[&str], insert_values: &[&str]) -> String {
    render_insert_sql(current_backend(), table, insert_columns, insert_values)
}

pub fn build_insert_if_absent_sql(
    table: &str,
    insert_columns: &[&str],
    insert_values: &[&str],
    conflict_columns: &[&str],
) -> String {
    render_insert_if_absent_sql(
        current_backend(),
        table,
        insert_columns,
        insert_values,
        conflict_columns,
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
    build_upsert_sql_with_managed_version(
        table,
        insert_columns,
        insert_values,
        conflict_columns,
        update_columns,
        update_updated_at,
        None,
    )
}

/// [`build_upsert_sql`] for an entity with a managed version column.
pub fn build_upsert_sql_with_managed_version(
    table: &str,
    insert_columns: &[&str],
    insert_values: &[&str],
    conflict_columns: &[&str],
    update_columns: &[&str],
    update_updated_at: bool,
    managed_version: Option<&str>,
) -> String {
    render_upsert_sql_with_managed_version(
        current_backend(),
        table,
        insert_columns,
        insert_values,
        conflict_columns,
        update_columns,
        update_updated_at,
        managed_version,
    )
}
type EntityMatcher<T> = Arc<dyn Fn(&T, chrono::NaiveDate) -> crate::Result<bool> + Send + Sync>;

struct PoolRef<'a, B: OrmBackend> {
    pool: &'a B::Pool,
}

impl<B: OrmBackend> PoolProvider<B> for PoolRef<'_, B> {
    fn pool(&self) -> &B::Pool {
        self.pool
    }
}

#[derive(Clone)]
struct ProjectionQueryState<P, B: OrmBackend>
where
    P: ReadProjection<B>,
{
    where_clauses: Vec<String>,
    values: Vec<SqlValue>,
    order_clauses: Vec<String>,
    limit: Option<i64>,
    max_limit: Option<i64>,
    requires_in_memory_filtering: bool,
    filter_is_valid: bool,
    _marker: PhantomData<(P, B)>,
}

/// Which projection read is being bounded.
#[derive(Clone, Copy)]
enum ProjectionRead {
    All,
    First,
    OptionalOne,
}

impl<P, B> ProjectionQueryState<P, B>
where
    B: OrmBackend,
    P: ReadProjection<B>,
{
    fn new() -> Self {
        Self {
            where_clauses: Vec::new(),
            values: Vec::new(),
            order_clauses: Vec::new(),
            limit: None,
            max_limit: None,
            requires_in_memory_filtering: false,
            filter_is_valid: true,
            _marker: PhantomData,
        }
    }

    fn filter(mut self, filter: P::Filter) -> Self {
        self.filter_is_valid &= filter.validate().is_ok();
        if filter.requires_in_memory_filtering(B::DIALECT) {
            self.requires_in_memory_filtering = true;
        } else {
            let (conditions, values) = filter.to_sql_conditions();
            self.where_clauses.extend(conditions);
            self.values.extend(values);
        }
        self
    }

    fn order_by(mut self, order: P::Order) -> Self {
        self.filter_is_valid &= order.validate_order().is_ok();
        if let Some(sort) = order.to_sort_expression() {
            self.order_clauses.push(sort.clause);
        }
        self
    }

    fn where_columns(mut self, columns: &[&str], values: Vec<SqlValue>) -> Self {
        debug_assert_eq!(columns.len(), values.len());
        self.where_clauses
            .extend(columns.iter().enumerate().map(|(index, column)| {
                format!(
                    "{} = {}",
                    B::DIALECT.quote_identifier_path(column),
                    B::DIALECT.placeholder(index + 1)
                )
            }));
        self.values.extend(values);
        self
    }

    fn limit(mut self, limit: i64) -> Self {
        self.limit = Some(limit);
        self
    }

    fn with_max_limit(mut self, maximum: i64) -> Self {
        self.max_limit = Some(maximum.max(0));
        self
    }

    /// Resolve the bound of one fetch.
    fn bound(
        &self,
        pagination_config: PaginationConfig,
        read: ProjectionRead,
    ) -> crate::Result<RepositoryBound> {
        let pagination_config = pagination_config.with_read_maximum(self.max_limit);
        match read {
            ProjectionRead::All => pagination_config.repository_bound(self.limit),
            ProjectionRead::First => Ok(pagination_config.repository_bound(self.limit)?.capped(1)),
            // The cardinality look-ahead ignores the limit and the configured
            // maximum, so a second matching row is never hidden.
            ProjectionRead::OptionalOne => Ok(RepositoryBound::Exact(2)),
        }
    }

    /// Render with an already resolved bound, applied exactly.
    fn render(
        &self,
        bound: RepositoryBound,
        visibility: Option<&FilterExpression>,
    ) -> RenderedQuery {
        let entity_default = <P::Entity as DatabaseEntity>::DEFAULT_SORT;
        let mut sorts = if self.order_clauses.is_empty() {
            vec![SortExpression {
                clause: entity_default.to_string(),
                values: Vec::new(),
            }]
        } else {
            self.order_clauses
                .iter()
                .cloned()
                .map(SortExpression::unbound)
                .collect::<Vec<_>>()
        };
        for primary_key in <P::Entity as DatabaseEntity>::PRIMARY_KEYS {
            if !sorts
                .iter()
                .any(|sort| sort_clause_orders_by_column(&sort.clause, primary_key))
            {
                sorts.push(SortExpression {
                    clause: format!("{} ASC", B::DIALECT.render_identifier_path(primary_key)),
                    values: Vec::new(),
                });
            }
        }
        let page = PageInput {
            limit: bound.sql_limit(),
            offset: None,
        };
        render_select_query(
            B::DIALECT,
            &SelectQuery {
                table: <P::Entity as DatabaseEntity>::TABLE_NAME,
                columns: P::COLUMNS
                    .iter()
                    .map(|column| (*column).to_string())
                    .collect(),
                filter: match (
                    filter_expression_from_raw_parts(&self.where_clauses, &self.values),
                    visibility,
                ) {
                    (Some(filter), Some(policy)) => {
                        Some(FilterExpression::And(vec![filter, policy.clone()]))
                    }
                    (filter, None) => filter,
                    (None, Some(policy)) => Some(policy.clone()),
                },
                sorts,
                pagination: Some(PaginationConfig::unbounded().resolve_page(Some(&page), false)),
                count_only: false,
            },
        )
    }

    fn validate(&self) -> crate::Result<()> {
        if !self.filter_is_valid {
            return Err(invalid_filter_input());
        }
        if self.requires_in_memory_filtering {
            return Err(sqlx::Error::Protocol(
                "projection filter requires full-entity in-memory evaluation and was rejected"
                    .to_string(),
            ));
        }
        Ok(())
    }

    fn decode(rows: Vec<B::Row>) -> crate::Result<Vec<P>> {
        rows.iter().map(P::from_row).collect()
    }
}

async fn ensure_projection_access<P, B>(
    db: &crate::db::Database<B>,
) -> crate::Result<super::ReadVisibility>
where
    B: OrmBackend,
    P: ReadProjection<B>,
{
    let metadata = P::Entity::metadata();
    db.ensure_entity_access(
        None,
        metadata.entity_name,
        metadata.read_policy,
        super::core::EntityAccessKind::Read,
        super::core::EntityAccessSurface::Repository,
    )
    .await
    .map_err(crate::graphql::errors::sqlx_error_from_graphql)?;
    for selected in P::FIELD_NAMES {
        let Some(field) = <P::Entity as Entity>::repository_field_policies()
            .iter()
            .find(|field| field.rust_name == *selected)
        else {
            return Err(sqlx::Error::Protocol(format!(
                "projection field metadata is missing for {}.{selected}",
                metadata.entity_name
            )));
        };
        db.ensure_repository_readable_field(
            None,
            metadata.entity_name,
            field.api_name,
            field.read_policy,
            None,
        )
        .await
        .map_err(crate::graphql::errors::sqlx_error_from_public)?;
    }
    let visibility = db
        .projection_read_visibility::<P>()
        .await
        .map_err(crate::graphql::errors::sqlx_error_from_graphql)?;
    if visibility.requires_residual_checks() {
        return Err(residual_visibility_refused(
            "projection reads are denied when an application row policy requires a full entity; use complete SQL visibility or explicit unrestricted visibility",
        ));
    }
    Ok(visibility)
}

/// Backend-neutral repository query for one macro-generated projection DTO.
pub struct ProjectionQuery<'a, P, B: OrmBackend = DefaultBackend>
where
    P: ReadProjection<B>,
{
    db: &'a crate::db::Database<B>,
    auth: Option<DbAuthContext>,
    state: ProjectionQueryState<P, B>,
}

impl<'a, P, B> ProjectionQuery<'a, P, B>
where
    B: OrmBackend,
    P: ReadProjection<B>,
{
    pub fn new(db: &'a crate::db::Database<B>) -> Self {
        Self {
            db,
            auth: None,
            state: ProjectionQueryState::new(),
        }
    }

    /// Apply backend-neutral transaction-style database auth to this read.
    pub fn with_auth(mut self, auth: Option<&DbAuthContext>) -> Self {
        self.auth = auth.cloned();
        self
    }

    pub fn filter(mut self, filter: P::Filter) -> Self {
        self.state = self.state.filter(filter);
        self
    }

    pub fn order_by(mut self, order: P::Order) -> Self {
        self.state = self.state.order_by(order);
        self
    }

    /// Add ordering terms in the order given: the first pair is the primary
    /// sort key. Each pair is one generated orderable field and a direction,
    /// and is applied exactly as one [`Self::order_by`] call, so the same
    /// orderable-field and read-policy checks apply and it composes with
    /// other ordering calls in call order.
    ///
    /// Use this, or chained `order_by` calls, when precedence matters: one
    /// generated `OrderByInput` value with several fields set always sorts in
    /// the entity's field declaration order.
    pub fn order_by_fields<K>(
        mut self,
        fields: impl IntoIterator<Item = (K, OrderDirection)>,
    ) -> Self
    where
        K: DatabaseOrderField<OrderBy = P::Order>,
    {
        for (field, direction) in fields {
            self = self.order_by(field.order(direction));
        }
        self
    }

    /// Read at most `limit` rows, exactly. A limit above the database's
    /// maximum page limit fails with `PAGE_LIMIT_EXCEEDED` instead of being
    /// reduced. Without a limit, `fetch_all` reads at most the maximum and
    /// fails with `PAGE_LIMIT_EXCEEDED` when more rows match.
    pub fn limit(mut self, limit: i64) -> Self {
        self.state = self.state.limit(limit);
        self
    }

    /// Override the database's maximum page limit for this read only.
    ///
    /// Trusted internal reads that carry their own bound use this instead of
    /// raising the maximum for every caller of the database. The read stays
    /// bounded: a `limit` above `maximum` still fails with
    /// `PAGE_LIMIT_EXCEEDED`, and an unlimited `fetch_all` fails closed when
    /// more than `maximum` rows match. Never pass a value a client chose
    /// without validating it first. A negative `maximum` is treated as `0`.
    pub fn with_max_limit(mut self, maximum: i64) -> Self {
        self.state = self.state.with_max_limit(maximum);
        self
    }

    #[doc(hidden)]
    pub fn __where_primary_key(mut self, values: Vec<SqlValue>) -> Self {
        self.state = self
            .state
            .where_columns(<P::Entity as DatabaseEntity>::PRIMARY_KEYS, values);
        self
    }

    #[doc(hidden)]
    pub fn __where_unique(mut self, index: usize, value: SqlValue) -> Self {
        let column = P::UNIQUE_COLUMNS
            .get(index)
            .expect("macro-generated projection unique-column index");
        self.state = self.state.where_columns(&[column], vec![value]);
        self
    }

    pub async fn fetch_all(self) -> crate::Result<Vec<P>> {
        let visibility = ensure_projection_access::<P, B>(self.db).await?;
        self.state.validate()?;
        let bound = self
            .state
            .bound(self.db.pagination_config(), ProjectionRead::All)?;
        let rendered = self.state.render(bound, visibility.predicate_expression());
        let rows = B::fetch_rows_with_auth(
            self.db.pool(),
            &rendered.sql,
            &rendered.values,
            self.auth.as_ref(),
        )
        .await?;
        self.db.observe_read(&rendered.sql, rows.len());
        bound.check_overflow(rows.len())?;
        ProjectionQueryState::<P, B>::decode(rows)
    }

    pub async fn fetch_first(self) -> crate::Result<Option<P>> {
        let visibility = ensure_projection_access::<P, B>(self.db).await?;
        self.state.validate()?;
        let bound = self
            .state
            .bound(self.db.pagination_config(), ProjectionRead::First)?;
        let rendered = self.state.render(bound, visibility.predicate_expression());
        let rows = B::fetch_rows_with_auth(
            self.db.pool(),
            &rendered.sql,
            &rendered.values,
            self.auth.as_ref(),
        )
        .await?;
        self.db.observe_read(&rendered.sql, rows.len());
        Ok(ProjectionQueryState::<P, B>::decode(rows)?
            .into_iter()
            .next())
    }

    pub async fn fetch_optional_one(self) -> crate::Result<Option<P>> {
        let visibility = ensure_projection_access::<P, B>(self.db).await?;
        self.state.validate()?;
        let bound = self
            .state
            .bound(self.db.pagination_config(), ProjectionRead::OptionalOne)?;
        let rendered = self.state.render(bound, visibility.predicate_expression());
        let rows = B::fetch_rows_with_auth(
            self.db.pool(),
            &rendered.sql,
            &rendered.values,
            self.auth.as_ref(),
        )
        .await?;
        self.db.observe_read(&rendered.sql, rows.len());
        let mut values = ProjectionQueryState::<P, B>::decode(rows)?;
        if values.len() > 1 {
            return Err(sqlx::Error::Protocol(
                "projection optional-one query matched more than one row".to_string(),
            ));
        }
        Ok(values.pop())
    }
}

#[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
/// Transaction-bound backend-neutral query for one projection DTO.
pub struct TransactionProjectionQuery<'ctx, 'tx, P, B: WriteBackend>
where
    P: ReadProjection<B>,
{
    context: &'ctx mut MutationContext<'tx, B>,
    state: ProjectionQueryState<P, B>,
}

#[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
impl<'ctx, 'tx, P, B> TransactionProjectionQuery<'ctx, 'tx, P, B>
where
    B: WriteBackend,
    P: ReadProjection<B>,
{
    pub(crate) fn new(context: &'ctx mut MutationContext<'tx, B>) -> Self {
        Self {
            context,
            state: ProjectionQueryState::new(),
        }
    }

    pub fn filter(mut self, filter: P::Filter) -> Self {
        self.state = self.state.filter(filter);
        self
    }

    pub fn order_by(mut self, order: P::Order) -> Self {
        self.state = self.state.order_by(order);
        self
    }

    /// Add ordering terms in the order given: the first pair is the primary
    /// sort key. Each pair is one generated orderable field and a direction,
    /// and is applied exactly as one [`Self::order_by`] call, so the same
    /// orderable-field and read-policy checks apply and it composes with
    /// other ordering calls in call order.
    ///
    /// Use this, or chained `order_by` calls, when precedence matters: one
    /// generated `OrderByInput` value with several fields set always sorts in
    /// the entity's field declaration order.
    pub fn order_by_fields<K>(
        mut self,
        fields: impl IntoIterator<Item = (K, OrderDirection)>,
    ) -> Self
    where
        K: DatabaseOrderField<OrderBy = P::Order>,
    {
        for (field, direction) in fields {
            self = self.order_by(field.order(direction));
        }
        self
    }

    /// Read at most `limit` rows, exactly. A limit above the database's
    /// maximum page limit fails with `PAGE_LIMIT_EXCEEDED` instead of being
    /// reduced. Without a limit, `fetch_all` reads at most the maximum and
    /// fails with `PAGE_LIMIT_EXCEEDED` when more rows match.
    pub fn limit(mut self, limit: i64) -> Self {
        self.state = self.state.limit(limit);
        self
    }

    /// Override the database's maximum page limit for this read only.
    ///
    /// Trusted internal reads that carry their own bound use this instead of
    /// raising the maximum for every caller of the database. The read stays
    /// bounded: a `limit` above `maximum` still fails with
    /// `PAGE_LIMIT_EXCEEDED`, and an unlimited `fetch_all` fails closed when
    /// more than `maximum` rows match. Never pass a value a client chose
    /// without validating it first. A negative `maximum` is treated as `0`.
    pub fn with_max_limit(mut self, maximum: i64) -> Self {
        self.state = self.state.with_max_limit(maximum);
        self
    }

    #[doc(hidden)]
    pub fn __where_primary_key(mut self, values: Vec<SqlValue>) -> Self {
        self.state = self
            .state
            .where_columns(<P::Entity as DatabaseEntity>::PRIMARY_KEYS, values);
        self
    }

    #[doc(hidden)]
    pub fn __where_unique(mut self, index: usize, value: SqlValue) -> Self {
        let column = P::UNIQUE_COLUMNS
            .get(index)
            .expect("macro-generated projection unique-column index");
        self.state = self.state.where_columns(&[column], vec![value]);
        self
    }

    async fn fetch(self, read: ProjectionRead) -> crate::Result<Vec<P>> {
        let visibility = ensure_projection_access::<P, B>(self.context.database()).await?;
        self.state.validate()?;
        let bound = self
            .state
            .bound(self.context.database().pagination_config(), read)?;
        let rendered = self.state.render(bound, visibility.predicate_expression());
        let rows = self
            .context
            .fetch_rows(&rendered.sql, &rendered.values)
            .await?;
        self.context
            .database()
            .observe_read(&rendered.sql, rows.len());
        bound.check_overflow(rows.len())?;
        ProjectionQueryState::<P, B>::decode(rows)
    }

    pub async fn fetch_all(self) -> crate::Result<Vec<P>> {
        self.fetch(ProjectionRead::All).await
    }

    pub async fn fetch_first(self) -> crate::Result<Option<P>> {
        Ok(self.fetch(ProjectionRead::First).await?.into_iter().next())
    }

    pub async fn fetch_optional_one(self) -> crate::Result<Option<P>> {
        let mut values = self.fetch(ProjectionRead::OptionalOne).await?;
        if values.len() > 1 {
            return Err(sqlx::Error::Protocol(
                "projection optional-one query matched more than one row".to_string(),
            ));
        }
        Ok(values.pop())
    }
}

pub struct EntityQuery<T, B: OrmBackend = DefaultBackend> {
    pub where_clauses: Vec<String>,
    pub values: Vec<SqlValue>,
    pub order_clauses: Vec<String>,
    pub(crate) order_values: Vec<Vec<SqlValue>>,
    pub page: Option<PageInput>,
    entity_matchers: Vec<EntityMatcher<T>>,
    filter_is_valid: bool,
    pub(crate) keyset_filter_identities: Vec<String>,
    _marker: PhantomData<(T, B)>,
}

impl<T, B: OrmBackend> Clone for EntityQuery<T, B> {
    fn clone(&self) -> Self {
        Self {
            where_clauses: self.where_clauses.clone(),
            values: self.values.clone(),
            order_clauses: self.order_clauses.clone(),
            order_values: self.order_values.clone(),
            page: self.page.clone(),
            entity_matchers: self.entity_matchers.clone(),
            filter_is_valid: self.filter_is_valid,
            keyset_filter_identities: self.keyset_filter_identities.clone(),
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
            order_values: Vec::new(),
            page: None,
            entity_matchers: Vec::new(),
            filter_is_valid: true,
            keyset_filter_identities: Vec::new(),
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
        self.filter_is_valid &= filter.validate().is_ok();
        if let Some(identity) = filter.keyset_filter_identity() {
            self.keyset_filter_identities.push(identity);
        }
        let (conds, values) = filter.to_sql_conditions();
        self.where_clauses.extend(conds);
        self.values.extend(values);
        self
    }

    pub fn filter_with_entity_matching<F>(mut self, filter: &F) -> Self
    where
        F: DatabaseFilter + Clone + Send + Sync + 'static,
    {
        self.filter_is_valid &= filter.validate().is_ok();
        if let Some(identity) = filter.keyset_filter_identity() {
            self.keyset_filter_identities.push(identity);
        }
        if filter.requires_in_memory_filtering(B::DIALECT) {
            let (conds, values) = filter.to_sql_prefilter_conditions(B::DIALECT);
            self.where_clauses.extend(conds);
            self.values.extend(values);
            let filter = filter.clone();
            self.entity_matchers
                .push(Arc::new(move |entity, calendar_today| {
                    filter.matches_entity_at(entity, calendar_today)
                }));
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
        self.filter_is_valid &= order.validate_order().is_ok();
        if let Some(sort) = order.to_sort_expression() {
            self.order_clauses.push(sort.clause);
            self.order_values.push(sort.values);
        }
        self
    }

    /// Add ordering terms in the order given: the first pair is the primary
    /// sort key. Each pair is one generated orderable field and a direction,
    /// and is applied exactly as one [`Self::order_by`] call, so the same
    /// orderable-field and read-policy checks apply and it composes with
    /// other ordering calls in call order.
    ///
    /// Use this, or chained `order_by` calls, when precedence matters: one
    /// generated `OrderByInput` value with several fields set always sorts in
    /// the entity's field declaration order.
    pub fn order_by_fields<K>(
        mut self,
        fields: impl IntoIterator<Item = (K, OrderDirection)>,
    ) -> Self
    where
        K: DatabaseOrderField,
    {
        for (field, direction) in fields {
            self = self.order_by(&field.order(direction));
        }
        self
    }

    /// Apply an order input while resolving any entity-owned bind parameters
    /// from the trusted GraphQL server context.
    pub fn order_by_with_context<O>(
        mut self,
        order: &O,
        ctx: &async_graphql::Context<'_>,
    ) -> async_graphql::Result<Self>
    where
        O: DatabaseOrderBy,
    {
        order
            .validate_order()
            .map_err(crate::graphql::errors::graphql_error_from_sqlx)?;
        if let Some(sort) = order.to_sort_expression_with_context(ctx)? {
            self.order_clauses.push(sort.clause);
            self.order_values.push(sort.values);
        }
        Ok(self)
    }

    pub fn default_order(mut self) -> Self {
        self.order_clauses.push(T::DEFAULT_SORT.to_string());
        self.order_values.push(Vec::new());
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
    ) -> crate::Result<SelectQuery> {
        self.ensure_filter_valid()?;
        let page = pagination_config.resolve_page(self.page.as_ref(), apply_default_limit);
        let mut sorts = self
            .order_clauses
            .iter()
            .enumerate()
            .map(|(index, clause)| SortExpression {
                clause: clause.clone(),
                values: self.order_values.get(index).cloned().unwrap_or_default(),
            })
            .collect::<Vec<_>>();
        for primary_key in T::PRIMARY_KEYS {
            if !sorts
                .iter()
                .any(|sort| sort_clause_orders_by_column(&sort.clause, primary_key))
            {
                sorts.push(SortExpression::unbound(format!(
                    "{} ASC",
                    B::DIALECT.render_identifier_path(primary_key)
                )));
            }
        }
        Ok(SelectQuery {
            table: T::TABLE_NAME,
            columns: T::column_names()
                .iter()
                .map(|column| (*column).to_string())
                .collect(),
            filter: filter_expression_from_raw_parts(&self.where_clauses, &self.values),
            sorts,
            pagination: if self.requires_in_memory_filtering() {
                None
            } else if page.limit.is_some() || page.offset > 0 {
                Some(page)
            } else {
                None
            },
            count_only: false,
        })
    }

    fn build_select_query(&self) -> crate::Result<SelectQuery> {
        self.build_select_query_with_config(PaginationConfig::default(), false)
    }

    pub(crate) fn ensure_filter_valid(&self) -> crate::Result<()> {
        if self.filter_is_valid {
            Ok(())
        } else {
            Err(invalid_filter_input())
        }
    }

    fn aggregate_column_sql(column: &str) -> crate::Result<String>
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
    ) -> crate::Result<AggregateQuery>
    where
        T: DatabaseSchema,
    {
        self.ensure_filter_valid()?;
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

    fn matches_entity(&self, entity: &T, calendar_today: chrono::NaiveDate) -> crate::Result<bool> {
        self.entity_matchers
            .iter()
            .try_fold(true, |matches, matcher| {
                if matches {
                    matcher(entity, calendar_today)
                } else {
                    Ok(false)
                }
            })
    }

    fn apply_in_memory_filtering(
        &self,
        rows: Vec<B::Row>,
        pagination_config: PaginationConfig,
        apply_default_limit: bool,
    ) -> crate::Result<Vec<T>> {
        let mut entities = rows
            .iter()
            .map(T::from_row)
            .collect::<Result<Vec<_>, _>>()?;

        if self.requires_in_memory_filtering() {
            let calendar_today =
                chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now()).date_naive();
            entities = entities
                .into_iter()
                .filter_map(
                    |entity| match self.matches_entity(&entity, calendar_today) {
                        Ok(true) => Some(Ok(entity)),
                        Ok(false) => None,
                        Err(error) => Some(Err(error)),
                    },
                )
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

    async fn fetch_unpaged_filtered<P>(&self, provider: &P) -> crate::Result<Vec<T>>
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
    ) -> crate::Result<Vec<T>>
    where
        P: PoolProvider<B> + ?Sized,
    {
        let mut query = self.clone();
        query.page = None;
        query.fetch_all_with_auth(provider, auth).await
    }

    pub async fn fetch_all<P>(&self, provider: &P) -> crate::Result<Vec<T>>
    where
        P: PoolProvider<B> + ?Sized,
    {
        let pagination_config = provider.pagination_config();
        let rendered = render_select_query(
            B::DIALECT,
            &self.build_select_query_with_config(pagination_config, false)?,
        );
        let rows = B::fetch_rows(provider.pool(), &rendered.sql, &rendered.values).await?;
        provider.observe_read(&rendered.sql, rows.len());
        self.apply_in_memory_filtering(rows, pagination_config, false)
    }

    pub async fn fetch_all_with_auth<P>(
        &self,
        provider: &P,
        auth: Option<&DbAuthContext>,
    ) -> crate::Result<Vec<T>>
    where
        P: PoolProvider<B> + ?Sized,
    {
        let pagination_config = provider.pagination_config();
        let rendered = render_select_query(
            B::DIALECT,
            &self.build_select_query_with_config(pagination_config, false)?,
        );
        let rows =
            B::fetch_rows_with_auth(provider.pool(), &rendered.sql, &rendered.values, auth).await?;
        provider.observe_read(&rendered.sql, rows.len());
        self.apply_in_memory_filtering(rows, pagination_config, false)
    }

    /// The `SELECT` of one page of candidate rows: the SQL filter, the order
    /// with its key tiebreak, and `offset`/`limit` applied in SQL even when
    /// residual in-memory predicates remain.
    fn candidate_select(&self, offset: i64, limit: Option<i64>) -> crate::Result<SelectQuery> {
        let mut query =
            self.build_select_query_with_config(PaginationConfig::unbounded(), false)?;
        query.pagination = (limit.is_some() || offset > 0).then_some(PaginationRequest {
            limit,
            offset: offset.max(0),
        });
        Ok(query)
    }

    /// Decode candidate rows, pairing each with whether it satisfies the
    /// residual in-memory predicates. Rows are never dropped, so a caller can
    /// count every candidate it read.
    fn decode_candidates(&self, rows: &[B::Row]) -> crate::Result<Vec<(T, bool)>> {
        let calendar_today =
            chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now()).date_naive();
        rows.iter()
            .map(|row| {
                let entity = T::from_row(row)?;
                let matches = self.matches_entity(&entity, calendar_today)?;
                Ok((entity, matches))
            })
            .collect()
    }

    /// Read one page of candidate rows, at most `limit` rows from database
    /// offset `offset`, each with whether it satisfies the residual in-memory
    /// predicates.
    pub(crate) async fn fetch_candidates_with_auth<P>(
        &self,
        provider: &P,
        auth: Option<&DbAuthContext>,
        offset: i64,
        limit: Option<i64>,
    ) -> crate::Result<Vec<(T, bool)>>
    where
        P: PoolProvider<B> + ?Sized,
    {
        let rendered = render_select_query(B::DIALECT, &self.candidate_select(offset, limit)?);
        let rows =
            B::fetch_rows_with_auth(provider.pool(), &rendered.sql, &rendered.values, auth).await?;
        provider.observe_read(&rendered.sql, rows.len());
        self.decode_candidates(&rows)
    }

    /// [`Self::fetch_candidates_with_auth`] on a pinned transaction.
    #[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
    pub(crate) async fn fetch_candidates_in_transaction(
        &self,
        context: &mut MutationContext<'_, B>,
        offset: i64,
        limit: Option<i64>,
    ) -> crate::Result<Vec<(T, bool)>>
    where
        B: WriteBackend,
    {
        let rendered = render_select_query(B::DIALECT, &self.candidate_select(offset, limit)?);
        let rows = context.fetch_rows(&rendered.sql, &rendered.values).await?;
        self.decode_candidates(&rows)
    }

    pub(crate) async fn fetch_all_with_auth_and_pagination<P>(
        &self,
        provider: &P,
        auth: Option<&DbAuthContext>,
        pagination_config: PaginationConfig,
    ) -> crate::Result<Vec<T>>
    where
        P: PoolProvider<B> + ?Sized,
    {
        let rendered = render_select_query(
            B::DIALECT,
            &self.build_select_query_with_config(pagination_config, false)?,
        );
        let rows =
            B::fetch_rows_with_auth(provider.pool(), &rendered.sql, &rendered.values, auth).await?;
        provider.observe_read(&rendered.sql, rows.len());
        self.apply_in_memory_filtering(rows, pagination_config, false)
    }

    pub async fn fetch_all_on<'e, E>(&self, executor: E) -> crate::Result<Vec<T>>
    where
        B: SqlxBackend,
        E: sqlx::Executor<'e, Database = <B as SqlxBackend>::Database> + Send + 'e,
    {
        self.fetch_all_on_with_pagination(executor, PaginationConfig::default())
            .await
    }

    /// [`Self::fetch_all_on`] with an explicit pagination configuration.
    pub(crate) async fn fetch_all_on_with_pagination<'e, E>(
        &self,
        executor: E,
        pagination: PaginationConfig,
    ) -> crate::Result<Vec<T>>
    where
        B: SqlxBackend,
        E: sqlx::Executor<'e, Database = <B as SqlxBackend>::Database> + Send + 'e,
    {
        let rendered = render_select_query(
            B::DIALECT,
            &self.build_select_query_with_config(pagination, false)?,
        );
        let rows = B::fetch_rows_on(executor, rendered.sql, rendered.values).await?;
        self.apply_in_memory_filtering(rows, pagination, false)
    }

    #[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
    #[doc(hidden)]
    pub async fn fetch_all_in_transaction(
        &self,
        context: &mut MutationContext<'_, B>,
    ) -> crate::Result<Vec<T>>
    where
        B: WriteBackend,
    {
        // In-transaction callers resolve their page before this call (against
        // the database's configuration, or as an internal `bound + 1`
        // look-ahead), so the page is applied exactly rather than re-clamped.
        let pagination = PaginationConfig::unbounded();
        let rendered = render_select_query(
            B::DIALECT,
            &self.build_select_query_with_config(pagination, false)?,
        );
        let rows = context.fetch_rows(&rendered.sql, &rendered.values).await?;
        self.apply_in_memory_filtering(rows, pagination, false)
    }

    /// Fetch rows used to authorize a subsequent mutation while acquiring the
    /// backend's portable write-decision lock on the same pinned transaction.
    #[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
    #[doc(hidden)]
    pub async fn fetch_all_for_write_in_transaction(
        &self,
        context: &mut MutationContext<'_, B>,
    ) -> crate::Result<Vec<T>>
    where
        B: WriteBackend,
    {
        let pagination = PaginationConfig::unbounded();
        let rendered = render_write_decision_select_query(
            B::DIALECT,
            &self.build_select_query_with_config(pagination, false)?,
        );
        let rows = context.fetch_rows(&rendered.sql, &rendered.values).await?;
        self.apply_in_memory_filtering(rows, pagination, false)
    }

    /// Fetch at most one write-decision row on the pinned transaction.
    #[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
    #[doc(hidden)]
    pub async fn fetch_one_for_write_in_transaction(
        &self,
        context: &mut MutationContext<'_, B>,
    ) -> crate::Result<Option<T>>
    where
        B: WriteBackend,
    {
        Ok(self
            .fetch_all_for_write_in_transaction(context)
            .await?
            .into_iter()
            .next())
    }

    /// Fetch one page of write-decision candidates for a generated bulk write:
    /// at most `limit` rows from database offset `offset`, ordered by the
    /// complete primary key, each with whether it satisfies the residual
    /// in-memory predicates.
    ///
    /// The page is limited in SQL even when residual predicates remain, so a
    /// caller never materializes more candidates than it asked for, and the
    /// primary-key order makes the limit portable and the lock order
    /// deterministic.
    #[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
    pub(crate) async fn fetch_write_candidates_in_transaction(
        &self,
        context: &mut MutationContext<'_, B>,
        offset: i64,
        limit: i64,
    ) -> crate::Result<Vec<(T, bool)>>
    where
        B: WriteBackend,
    {
        let mut query = self.clone();
        query.order_clauses = vec![
            T::PRIMARY_KEYS
                .iter()
                .map(|column| B::DIALECT.quote_identifier_path(column))
                .collect::<Vec<_>>()
                .join(", "),
        ];
        query.order_values = Vec::new();
        let rendered = render_write_decision_select_query(
            B::DIALECT,
            &query.candidate_select(offset, Some(limit.max(0)))?,
        );
        let rows = context.fetch_rows(&rendered.sql, &rendered.values).await?;
        context.database().observe_read(&rendered.sql, rows.len());
        query.decode_candidates(&rows)
    }

    #[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
    #[doc(hidden)]
    pub async fn fetch_bounded_mutation_sentinel_in_transaction(
        &self,
        context: &mut MutationContext<'_, B>,
        limit: MutationLimit,
    ) -> crate::Result<Vec<T>>
    where
        B: WriteBackend,
    {
        if self.requires_in_memory_filtering() {
            return Err(crate::graphql::errors::sqlx_error_from_public(
                crate::graphql::errors::OrmPublicError::new(
                    crate::graphql::errors::OrmErrorCode::InvalidInput,
                )
                .with_internal(
                    "bounded mutations require a filter rendered completely by the database",
                ),
            ));
        }

        let sentinel_limit = i64::from(limit.get()).checked_add(1).ok_or_else(|| {
            crate::graphql::errors::sqlx_error_from_public(
                crate::graphql::errors::OrmPublicError::new(
                    crate::graphql::errors::OrmErrorCode::InternalError,
                )
                .with_internal("bounded mutation sentinel limit overflow"),
            )
        })?;
        let mut query = self.clone();
        query.order_clauses = vec![
            T::PRIMARY_KEYS
                .iter()
                .map(|column| B::DIALECT.quote_identifier_path(column))
                .collect::<Vec<_>>()
                .join(", "),
        ];
        query.page = Some(PageInput {
            limit: Some(sentinel_limit),
            offset: Some(0),
        });
        let pagination = PaginationConfig::unbounded();
        let rendered = render_write_decision_select_query(
            B::DIALECT,
            &query.build_select_query_with_config(pagination, false)?,
        );
        let rows = context.fetch_rows(&rendered.sql, &rendered.values).await?;
        query.apply_in_memory_filtering(rows, pagination, false)
    }

    pub async fn fetch_one<P>(&self, provider: &P) -> crate::Result<Option<T>>
    where
        P: PoolProvider<B> + ?Sized,
    {
        Ok(self.fetch_all(provider).await?.into_iter().next())
    }

    pub async fn fetch_one_with_auth<P>(
        &self,
        provider: &P,
        auth: Option<&DbAuthContext>,
    ) -> crate::Result<Option<T>>
    where
        P: PoolProvider<B> + ?Sized,
    {
        Ok(self
            .fetch_all_with_auth(provider, auth)
            .await?
            .into_iter()
            .next())
    }

    pub async fn fetch_one_on<'e, E>(&self, executor: E) -> crate::Result<Option<T>>
    where
        B: SqlxBackend,
        E: sqlx::Executor<'e, Database = <B as SqlxBackend>::Database> + Send + 'e,
    {
        Ok(self.fetch_all_on(executor).await?.into_iter().next())
    }

    #[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
    #[doc(hidden)]
    pub async fn fetch_one_in_transaction(
        &self,
        context: &mut MutationContext<'_, B>,
    ) -> crate::Result<Option<T>>
    where
        B: WriteBackend,
    {
        Ok(self
            .fetch_all_in_transaction(context)
            .await?
            .into_iter()
            .next())
    }

    pub async fn count<P>(&self, provider: &P) -> crate::Result<i64>
    where
        P: PoolProvider<B> + ?Sized,
    {
        if self.requires_in_memory_filtering() {
            return Ok(self.fetch_unpaged_filtered(provider).await?.len() as i64);
        }
        let mut query = self.build_select_query()?;
        query.count_only = true;
        query.pagination = None;
        query.sorts.clear();
        let rendered = render_select_query(B::DIALECT, &query);
        let rows = B::fetch_rows(provider.pool(), &rendered.sql, &rendered.values).await?;
        let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
        B::try_get_i64(row, "count")
    }

    pub async fn count_column<P>(&self, provider: &P, column: &str) -> crate::Result<i64>
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

    pub async fn max_i64<P>(&self, provider: &P, column: &str) -> crate::Result<Option<i64>>
    where
        P: PoolProvider<B> + ?Sized,
        T: DatabaseSchema,
    {
        self.aggregate_optional_i64(provider, AggregateFunction::Max, column)
            .await
    }

    pub async fn min_i64<P>(&self, provider: &P, column: &str) -> crate::Result<Option<i64>>
    where
        P: PoolProvider<B> + ?Sized,
        T: DatabaseSchema,
    {
        self.aggregate_optional_i64(provider, AggregateFunction::Min, column)
            .await
    }

    pub async fn max_f64<P>(&self, provider: &P, column: &str) -> crate::Result<Option<f64>>
    where
        P: PoolProvider<B> + ?Sized,
        T: DatabaseSchema,
    {
        self.aggregate_optional_f64(provider, AggregateFunction::Max, column)
            .await
    }

    pub async fn min_f64<P>(&self, provider: &P, column: &str) -> crate::Result<Option<f64>>
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
    ) -> crate::Result<Option<i64>>
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
    ) -> crate::Result<Option<f64>>
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
    ) -> crate::Result<i64>
    where
        P: PoolProvider<B> + ?Sized,
    {
        if self.requires_in_memory_filtering() {
            return Ok(self
                .fetch_unpaged_filtered_with_auth(provider, auth)
                .await?
                .len() as i64);
        }
        let mut query = self.build_select_query()?;
        query.count_only = true;
        query.pagination = None;
        query.sorts.clear();
        let rendered = render_select_query(B::DIALECT, &query);
        let rows =
            B::fetch_rows_with_auth(provider.pool(), &rendered.sql, &rendered.values, auth).await?;
        provider.observe_read(&rendered.sql, rows.len());
        let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
        B::try_get_i64(row, "count")
    }

    pub async fn count_column_with_auth<P>(
        &self,
        provider: &P,
        auth: Option<&DbAuthContext>,
        column: &str,
    ) -> crate::Result<i64>
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
        provider.observe_read(&rendered.sql, rows.len());
        let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
        B::try_get_i64(row, "__gom_aggregate")
    }

    pub async fn max_i64_with_auth<P>(
        &self,
        provider: &P,
        auth: Option<&DbAuthContext>,
        column: &str,
    ) -> crate::Result<Option<i64>>
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
    ) -> crate::Result<Option<i64>>
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
    ) -> crate::Result<Option<i64>>
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
        provider.observe_read(&rendered.sql, rows.len());
        let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
        B::try_get_optional_i64(row, "__gom_aggregate")
    }

    pub async fn count_on<'e, E>(&self, executor: E) -> crate::Result<i64>
    where
        B: SqlxBackend,
        E: sqlx::Executor<'e, Database = <B as SqlxBackend>::Database> + Send + 'e,
    {
        if self.requires_in_memory_filtering() {
            let mut query = self.clone();
            query.page = None;
            return Ok(query.fetch_all_on(executor).await?.len() as i64);
        }
        let mut query = self.build_select_query()?;
        query.count_only = true;
        query.pagination = None;
        query.sorts.clear();
        let rendered = render_select_query(B::DIALECT, &query);
        let rows = B::fetch_rows_on(executor, rendered.sql, rendered.values).await?;
        let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
        B::try_get_i64(row, "count")
    }

    #[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
    #[doc(hidden)]
    pub async fn count_in_transaction(
        &self,
        context: &mut MutationContext<'_, B>,
    ) -> crate::Result<i64>
    where
        B: WriteBackend,
    {
        self.count_with_visibility_in_transaction(context, None)
            .await
    }

    #[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
    pub(crate) async fn count_with_visibility_in_transaction(
        &self,
        context: &mut MutationContext<'_, B>,
        visibility: Option<&FilterExpression>,
    ) -> crate::Result<i64>
    where
        B: WriteBackend,
    {
        if self.requires_in_memory_filtering() {
            if visibility.is_some() {
                return Err(sqlx::Error::Protocol(
                    "SQL-authorized counts require database-visible query filters".to_string(),
                ));
            }
            let mut query = self.clone();
            query.page = None;
            return Ok(query.fetch_all_in_transaction(context).await?.len() as i64);
        }
        let mut query = self.build_select_query()?;
        if let Some(predicate) = visibility {
            query.filter = Some(match query.filter {
                Some(filter) => FilterExpression::And(vec![filter, predicate.clone()]),
                None => predicate.clone(),
            });
        }
        query.count_only = true;
        query.pagination = None;
        query.sorts.clear();
        let rendered = render_select_query(B::DIALECT, &query);
        let rows = context.fetch_rows(&rendered.sql, &rendered.values).await?;
        let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
        B::try_get_i64(row, "count")
    }

    /// Build a delete statement, rendering an invalid filter as a predicate
    /// that cannot match any row.
    pub fn build_delete_sql(&self) -> (String, Vec<SqlValue>) {
        self.try_build_delete_sql().unwrap_or_else(|_| {
            let rendered = render_delete_query(
                B::DIALECT,
                &DeleteQuery {
                    table: T::TABLE_NAME,
                    filter: Some(FilterExpression::trusted_fragment("1 = 0", Vec::new())),
                },
            );
            (rendered.sql, rendered.values)
        })
    }

    /// Validate the accumulated filter and build a delete statement.
    pub fn try_build_delete_sql(&self) -> crate::Result<(String, Vec<SqlValue>)> {
        self.ensure_filter_valid()?;
        let rendered = render_delete_query(
            B::DIALECT,
            &DeleteQuery {
                table: T::TABLE_NAME,
                filter: filter_expression_from_raw_parts(&self.where_clauses, &self.values),
            },
        );
        Ok((rendered.sql, rendered.values))
    }

    pub async fn fetch_connection<P>(&self, provider: &P) -> crate::Result<Connection<T>>
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
        let mut count_query = self.build_select_query()?;
        count_query.count_only = true;
        count_query.pagination = None;
        count_query.sorts.clear();
        let count_rendered = render_select_query(B::DIALECT, &count_query);
        let row_rendered = render_select_query(
            B::DIALECT,
            &self.build_select_query_with_config(pagination_config, true)?,
        );
        let (count_rows, rows) = super::fetch_rows_pair_with_auth::<B>(
            provider.pool(),
            &count_rendered.sql,
            &count_rendered.values,
            &row_rendered.sql,
            &row_rendered.values,
            None,
        )
        .await?;
        provider.observe_read(&count_rendered.sql, count_rows.len());
        provider.observe_read(&row_rendered.sql, rows.len());
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

    /// Fetch only selected connection components. Residual entity filters retain
    /// the complete read path; callers must apply residual authorization first.
    pub async fn fetch_connection_selected_with_auth<P>(
        &self,
        provider: &P,
        auth: Option<&DbAuthContext>,
        selection: crate::graphql::pagination::ConnectionSelection,
    ) -> crate::Result<Connection<T>>
    where
        P: PoolProvider<B> + ?Sized,
    {
        self.ensure_filter_valid()?;
        if self.requires_in_memory_filtering() || (selection.rows && selection.count) {
            return self.fetch_connection_with_auth(provider, auth).await;
        }
        let config = provider.pagination_config();
        let page = config.resolve_page(self.page.as_ref(), true);
        let offset = page.offset.max(0);
        let limit = page.limit;
        let needs_rows = selection.rows || (selection.has_next_page && !selection.count);
        let mut nodes = Vec::new();
        let mut total = None;
        if selection.count {
            let mut query = self.build_select_query()?;
            query.count_only = true;
            query.pagination = None;
            query.sorts.clear();
            let rendered = render_select_query(B::DIALECT, &query);
            let rows = super::fetch_rows_with_auth::<B>(
                provider.pool(),
                &rendered.sql,
                &rendered.values,
                auth,
            )
            .await?;
            provider.observe_read(&rendered.sql, rows.len());
            total = Some(B::try_get_i64(
                rows.first().ok_or(sqlx::Error::RowNotFound)?,
                "count",
            )?);
        }
        if needs_rows {
            let mut query = self.build_select_query_with_config(config, true)?;
            if !selection.count
                && selection.has_next_page
                && let Some(pagination) = &mut query.pagination
            {
                pagination.limit = pagination.limit.map(|n| n.saturating_add(1));
            }
            let rendered = render_select_query(B::DIALECT, &query);
            let rows = super::fetch_rows_with_auth::<B>(
                provider.pool(),
                &rendered.sql,
                &rendered.values,
                auth,
            )
            .await?;
            provider.observe_read(&rendered.sql, rows.len());
            nodes = rows
                .iter()
                .map(T::from_row)
                .collect::<Result<Vec<_>, _>>()?;
        }
        let page_len = limit.map_or(nodes.len() as i64, |n| (nodes.len() as i64).min(n.max(0)));
        let has_next_page = total.map_or_else(
            || limit.is_some_and(|n| nodes.len() as i64 > n),
            |total| {
                offset.saturating_add(limit.map_or((total - offset).max(0), |n| {
                    n.max(0).min((total - offset).max(0))
                })) < total
            },
        );
        nodes.truncate(page_len as usize);
        let edges: Vec<_> = nodes
            .into_iter()
            .enumerate()
            .map(|(i, node)| Edge {
                node,
                cursor: encode_cursor(offset + i as i64),
            })
            .collect();
        Ok(Connection {
            page_info: PageInfo {
                has_next_page,
                has_previous_page: offset > 0,
                start_cursor: edges.first().map(|e| e.cursor.clone()),
                end_cursor: edges.last().map(|e| e.cursor.clone()),
                total_count: total,
            },
            edges,
        })
    }

    pub async fn fetch_connection_with_auth<P>(
        &self,
        provider: &P,
        auth: Option<&DbAuthContext>,
    ) -> crate::Result<Connection<T>>
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
        let mut count_query = self.build_select_query()?;
        count_query.count_only = true;
        count_query.pagination = None;
        count_query.sorts.clear();
        let count_rendered = render_select_query(B::DIALECT, &count_query);
        let pagination_config = provider.pagination_config();
        let row_rendered = render_select_query(
            B::DIALECT,
            &self.build_select_query_with_config(pagination_config, true)?,
        );
        let (count_rows, rows) = super::fetch_rows_pair_with_auth::<B>(
            provider.pool(),
            &count_rendered.sql,
            &count_rendered.values,
            &row_rendered.sql,
            &row_rendered.values,
            auth,
        )
        .await?;
        provider.observe_read(&count_rendered.sql, count_rows.len());
        provider.observe_read(&row_rendered.sql, rows.len());
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
    max_limit: Option<i64>,
    _marker: PhantomData<(W, O, B)>,
}

/// Largest `limit` the generated `search_similar` helper accepts.
pub const SIMILAR_SEARCH_MAX_LIMIT: i64 = 1_000;

/// Candidate rows `search_similar` reads per requested result, so fuzzy
/// filtering can still fill the requested page.
pub const SIMILAR_SEARCH_CANDIDATE_FACTOR: i64 = 5;

/// Check a generated `search_similar` limit and return its candidate bound.
///
/// # Errors
///
/// Returns `INVALID_INPUT` for a negative limit and `PAGE_LIMIT_EXCEEDED` for
/// a limit above [`SIMILAR_SEARCH_MAX_LIMIT`].
#[doc(hidden)]
pub fn similar_search_candidate_limit(limit: i64) -> crate::Result<i64> {
    use crate::graphql::errors::{OrmErrorCode, OrmPublicError, sqlx_error_from_public};
    if limit < 0 {
        return Err(sqlx_error_from_public(OrmPublicError::with_message(
            OrmErrorCode::InvalidInput,
            "requested limit must not be negative",
        )));
    }
    if limit > SIMILAR_SEARCH_MAX_LIMIT {
        return Err(sqlx_error_from_public(OrmPublicError::with_message(
            OrmErrorCode::PageLimitExceeded,
            format!(
                "requested limit {limit} exceeds the similar-search maximum {SIMILAR_SEARCH_MAX_LIMIT}"
            ),
        )));
    }
    limit
        .checked_mul(SIMILAR_SEARCH_CANDIDATE_FACTOR)
        .ok_or_else(|| {
            sqlx_error_from_public(OrmPublicError::with_message(
                OrmErrorCode::PageLimitExceeded,
                "similar-search candidate bound overflow",
            ))
        })
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
            max_limit: None,
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

    /// Add ordering terms in the order given: the first pair is the primary
    /// sort key. Each pair is one generated orderable field and a direction,
    /// and is applied exactly as one [`Self::order_by`] call, so the same
    /// orderable-field and read-policy checks apply and it composes with
    /// other ordering calls in call order.
    ///
    /// Use this, or chained `order_by` calls, when precedence matters: one
    /// generated `OrderByInput` value with several fields set always sorts in
    /// the entity's field declaration order.
    pub fn order_by_fields<K>(
        mut self,
        fields: impl IntoIterator<Item = (K, OrderDirection)>,
    ) -> Self
    where
        O: DatabaseOrderBy,
        K: DatabaseOrderField<OrderBy = O>,
    {
        for (field, direction) in fields {
            self = self.order_by(field.order(direction));
        }
        self
    }

    /// Read at most `limit` rows, exactly. A pool-bound query cannot see a
    /// `Database` configuration, so a limit above
    /// [`PaginationConfig::DEFAULT_MAX_LIMIT`] fails with `PAGE_LIMIT_EXCEEDED`
    /// instead of being reduced. Without a limit the read is unbounded.
    pub fn limit(mut self, limit: i64) -> Self {
        let mut page = self.query.page.unwrap_or_default();
        page.limit = Some(limit);
        self.query.page = Some(page);
        self
    }

    /// Override [`PaginationConfig::DEFAULT_MAX_LIMIT`] for this read only.
    ///
    /// Trusted internal reads that carry their own bound use this. A
    /// [`Self::limit`] above `maximum` still fails with `PAGE_LIMIT_EXCEEDED`.
    /// Never pass a value a client chose without validating it first. A
    /// negative `maximum` is treated as `0`.
    pub fn with_max_limit(mut self, maximum: i64) -> Self {
        self.max_limit = Some(maximum.max(0));
        self
    }

    fn check_limit(&self) -> crate::Result<()> {
        if let Some(limit) = self.query.page.as_ref().and_then(|page| page.limit) {
            PaginationConfig::default()
                .with_read_maximum(self.max_limit)
                .checked_explicit_limit(limit)
                .map_err(crate::graphql::errors::sqlx_error_from_public)?;
        }
        Ok(())
    }

    pub fn offset(mut self, offset: i64) -> Self {
        let mut page = self.query.page.unwrap_or_default();
        page.offset = Some(offset);
        self.query.page = Some(page);
        self
    }

    pub async fn fetch_all(self) -> crate::Result<Vec<T>> {
        self.check_limit()?;
        // The limit is checked above, so the read applies it exactly.
        self.query
            .fetch_all_with_auth_and_pagination(
                &PoolRef { pool: self.pool },
                None,
                PaginationConfig::unbounded(),
            )
            .await
    }

    pub async fn fetch_all_on<'e, E>(self, executor: E) -> crate::Result<Vec<T>>
    where
        B: SqlxBackend,
        E: sqlx::Executor<'e, Database = <B as SqlxBackend>::Database> + Send + 'e,
    {
        self.check_limit()?;
        self.query
            .fetch_all_on_with_pagination(executor, PaginationConfig::unbounded())
            .await
    }

    pub async fn fetch_one(self) -> crate::Result<Option<T>> {
        Ok(self.fetch_all().await?.into_iter().next())
    }

    pub async fn fetch_one_on<'e, E>(self, executor: E) -> crate::Result<Option<T>>
    where
        B: SqlxBackend,
        E: sqlx::Executor<'e, Database = <B as SqlxBackend>::Database> + Send + 'e,
    {
        self.check_limit()?;
        Ok(self
            .query
            .fetch_all_on_with_pagination(executor, PaginationConfig::unbounded())
            .await?
            .into_iter()
            .next())
    }

    pub async fn count(self) -> crate::Result<i64> {
        self.query.count(&PoolRef { pool: self.pool }).await
    }

    pub async fn count_column(self, column: &str) -> crate::Result<i64>
    where
        T: DatabaseSchema,
    {
        self.query
            .count_column(&PoolRef { pool: self.pool }, column)
            .await
    }

    pub async fn max_i64(self, column: &str) -> crate::Result<Option<i64>>
    where
        T: DatabaseSchema,
    {
        self.query
            .max_i64(&PoolRef { pool: self.pool }, column)
            .await
    }

    pub async fn min_i64(self, column: &str) -> crate::Result<Option<i64>>
    where
        T: DatabaseSchema,
    {
        self.query
            .min_i64(&PoolRef { pool: self.pool }, column)
            .await
    }

    pub async fn max_f64(self, column: &str) -> crate::Result<Option<f64>>
    where
        T: DatabaseSchema,
    {
        self.query
            .max_f64(&PoolRef { pool: self.pool }, column)
            .await
    }

    pub async fn min_f64(self, column: &str) -> crate::Result<Option<f64>>
    where
        T: DatabaseSchema,
    {
        self.query
            .min_f64(&PoolRef { pool: self.pool }, column)
            .await
    }

    pub async fn count_on<'e, E>(self, executor: E) -> crate::Result<i64>
    where
        B: SqlxBackend,
        E: sqlx::Executor<'e, Database = <B as SqlxBackend>::Database> + Send + 'e,
    {
        self.query.count_on(executor).await
    }

    pub async fn exists(self) -> crate::Result<bool> {
        Ok(self.count().await? > 0)
    }

    pub async fn exists_on<'e, E>(self, executor: E) -> crate::Result<bool>
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

    pub async fn count(self) -> crate::Result<i64> {
        self.query.count(&PoolRef { pool: self.pool }).await
    }

    pub async fn count_on<'e, E>(self, executor: E) -> crate::Result<i64>
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
    filter_is_valid: bool,
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
            filter_is_valid: true,
            _marker: PhantomData,
        }
    }

    pub fn filter(mut self, filter: &W) -> Self {
        self.filter_is_valid &= filter.validate().is_ok();
        let (conds, values) = filter.to_sql_conditions();
        self.filters.extend(conds);
        self.values.extend(values);
        self
    }

    pub async fn count(self) -> crate::Result<i64> {
        if !self.filter_is_valid {
            return Err(invalid_filter_input());
        }
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

    pub async fn count_on<'e, E>(self, executor: E) -> crate::Result<i64>
    where
        B: SqlxBackend,
        E: sqlx::Executor<'e, Database = <B as SqlxBackend>::Database> + Send + 'e,
    {
        if !self.filter_is_valid {
            return Err(invalid_filter_input());
        }
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

#[cfg(test)]
mod grouped_aggregate_tests {
    use super::*;

    const OPERATORS: &[super::super::GraphqlAggregateOperator] = &[
        super::super::GraphqlAggregateOperator::Count,
        super::super::GraphqlAggregateOperator::Min,
        super::super::GraphqlAggregateOperator::Max,
        super::super::GraphqlAggregateOperator::Sum,
    ];

    fn field(name: &'static str, kind: AggregateFieldKind) -> AggregateFieldRef {
        AggregateFieldRef::generated(name, name, name, kind, false, true, OPERATORS)
    }

    #[test]
    fn postgres_page_comparisons_bind_ascii_folded_boundaries_and_exact_groups() {
        let query = GroupedAggregateSqlQuery {
            table: "history",
            groups: vec![field("event", AggregateFieldKind::Text)],
            metrics: vec![],
            filter: Some(FilterExpression::trusted_fragment(
                "tenant = ?",
                vec![SqlValue::String("alpha".into())],
            )),
            group_limit: 101,
        };
        let cursor = AggregateGroupCursor {
            format_version: 1,
            fingerprint: "a".repeat(64),
            key: Some("Event".into()),
        };
        let rendered = render_grouped_aggregate_query_page(
            DatabaseBackend::Postgres,
            &query,
            Some((AggregateGroupOrder::SqliteNoCase, Some(&cursor))),
        );
        assert!(
            rendered
                .sql
                .contains("GROUP BY CAST(\"event\" AS TEXT) COLLATE pg_catalog.\"C\" HAVING")
        );
        assert!(!rendered.sql.contains("NOCASE"));
        assert!(!rendered.sql.contains("lower("));
        assert!(rendered.sql.contains("translate(($2)::pg_catalog.text"));
        assert!(rendered.sql.contains("translate(($3)::pg_catalog.text"));
        assert!(rendered.sql.contains(" > $4"));
        assert_eq!(rendered.values.len(), 4);
        assert!(rendered.sql.ends_with("LIMIT 101"));
    }

    #[test]
    fn write_decision_selects_use_backend_specific_locks() {
        let query = SelectQuery {
            table: "work_records",
            columns: vec!["id".to_string(), "owner_id".to_string()],
            filter: None,
            sorts: vec![SortExpression {
                clause: "id ASC".to_string(),
                values: Vec::new(),
            }],
            pagination: Some(PaginationRequest {
                limit: Some(10),
                offset: 0,
            }),
            count_only: false,
        };

        let sqlite = render_write_decision_select_query(DatabaseBackend::Sqlite, &query);
        let postgres = render_write_decision_select_query(DatabaseBackend::Postgres, &query);
        let mssql = render_write_decision_select_query(DatabaseBackend::Mssql, &query);

        assert!(!sqlite.sql.contains("FOR UPDATE"));
        assert!(!sqlite.sql.contains("UPDLOCK"));
        assert!(postgres.sql.ends_with("LIMIT 10 FOR UPDATE"));
        assert!(
            mssql
                .sql
                .contains("FROM work_records WITH (UPDLOCK, HOLDLOCK)")
        );
    }

    #[test]
    fn grouped_renderer_widens_integral_sum_and_limits_groups() {
        let query = GroupedAggregateSqlQuery {
            table: "work_records",
            groups: vec![field("team", AggregateFieldKind::Text)],
            metrics: vec![AggregateExpression {
                operator: super::super::GraphqlAggregateOperator::Sum,
                field: Some(field("units", AggregateFieldKind::Integral)),
            }],
            filter: None,
            group_limit: 25,
        };
        let sqlite = render_grouped_aggregate_query(DatabaseBackend::Sqlite, &query);
        let postgres = render_grouped_aggregate_query(DatabaseBackend::Postgres, &query);
        let mssql = render_grouped_aggregate_query(DatabaseBackend::Mssql, &query);
        assert!(sqlite.sql.contains("SUM(\"units\")"));
        assert!(postgres.sql.contains("NUMERIC(38,0)"));
        assert!(mssql.sql.contains("DECIMAL(38,0)"));
        assert!(sqlite.sql.ends_with("LIMIT 25"));
        assert!(mssql.sql.ends_with("OFFSET 0 ROWS FETCH NEXT 25 ROWS ONLY"));
    }

    #[test]
    fn grouped_renderer_uses_collision_free_aliases_and_explicit_null_ordering() {
        let query = GroupedAggregateSqlQuery {
            table: "work_records",
            groups: vec![
                field("team", AggregateFieldKind::Text),
                field("category", AggregateFieldKind::Text),
            ],
            metrics: vec![
                AggregateExpression {
                    operator: super::super::GraphqlAggregateOperator::Count,
                    field: None,
                },
                AggregateExpression {
                    operator: super::super::GraphqlAggregateOperator::Max,
                    field: Some(field("units", AggregateFieldKind::Integral)),
                },
            ],
            filter: None,
            group_limit: 10,
        };
        let rendered = render_grouped_aggregate_query(DatabaseBackend::Postgres, &query);
        assert!(rendered.sql.contains("__gom_group_000"));
        assert!(rendered.sql.contains("__gom_group_001"));
        assert!(rendered.sql.contains("__gom_metric_000"));
        assert!(rendered.sql.contains("__gom_metric_001"));
        assert!(
            rendered
                .sql
                .contains("CASE WHEN \"team\" IS NULL THEN 0 ELSE 1 END ASC")
        );
    }
}

#[cfg(test)]
mod visible_row_scan_tests {
    use super::*;

    #[test]
    fn key_tiebreak_needs_a_direct_key_term() {
        // A direct key term, in any quoting, qualification or direction.
        for clause in [
            "id ASC",
            "\"id\" DESC",
            "[id] ASC",
            "`id`",
            "t.\"id\" DESC NULLS LAST",
            "rank ASC, id ASC",
            "\"rank\" desc, \"id\" asc",
        ] {
            assert!(sort_clause_orders_by_column(clause, "id"), "{clause}");
            assert!(sort_clause_orders_by_column(clause, "\"id\""), "{clause}");
        }
        // Expressions that only mention the key do not order by it.
        for clause in [
            "CASE WHEN id > 0 THEN 0 ELSE 1 END ASC",
            "LOWER(id) ASC",
            "id + 1 ASC",
            "COALESCE(label, id) DESC",
            "CASE WHEN rank IN (1, 2) THEN id END ASC, rank ASC",
            "'id' ASC",
            "grid ASC",
        ] {
            assert!(!sort_clause_orders_by_column(clause, "id"), "{clause}");
        }
    }

    #[test]
    fn qualified_key_terms_allow_spaces_quotes_and_brackets() {
        // SQLite, PostgreSQL and SQL Server spellings of a qualified key term.
        for clause in [
            "page_rows . id DESC",
            "page_rows.id DESC",
            "main . page_rows . id ASC",
            "\"page_rows\" . \"id\" DESC",
            "\"public\".\"page_rows\".\"id\"",
            "[dbo] . [page_rows] . [id] DESC",
            "[page_rows].[id ] ASC",
            "rank ASC, page_rows . id DESC",
        ] {
            assert!(sort_clause_orders_by_column(clause, "id"), "{clause}");
        }
        // Quoted names keep their exact text and escapes.
        assert!(sort_clause_orders_by_column(
            "\"odd\"\"key\" ASC",
            "odd\"key"
        ));
        assert!(sort_clause_orders_by_column("[odd]]key] ASC", "odd]key"));
        assert!(!sort_clause_orders_by_column("\"id \" ASC", "id"));
        // Expressions stay expressions, however they are spaced.
        for clause in [
            "page_rows . id + 1 DESC",
            "page_rows . LOWER(id) ASC",
            "page_rows . . id ASC",
            ". id ASC",
            "page_rows . ASC",
            "\"page_rows\" . \"id ASC",
        ] {
            assert!(!sort_clause_orders_by_column(clause, "id"), "{clause}");
        }
    }

    #[test]
    fn composite_keys_and_nullable_sort_columns_keep_missing_key_terms() {
        // One component of a composite key present: only the other is missing.
        let clause = "\"tenant\" ASC";
        assert!(sort_clause_orders_by_column(clause, "tenant"));
        assert!(!sort_clause_orders_by_column(clause, "id"));
        // A nullable sort column orders by itself, not by the key.
        let clause = "\"label\" ASC NULLS FIRST";
        assert!(sort_clause_orders_by_column(clause, "label"));
        assert!(!sort_clause_orders_by_column(clause, "id"));
    }

    #[test]
    fn an_unlimited_offset_renders_on_every_dialect() {
        assert_eq!(
            DatabaseBackend::Sqlite.render_pagination(None, 5),
            " LIMIT -1 OFFSET 5"
        );
        assert_eq!(
            DatabaseBackend::Postgres.render_pagination(None, 5),
            " OFFSET 5"
        );
        assert_eq!(
            DatabaseBackend::Mssql.render_pagination(None, 5),
            " OFFSET 5 ROWS"
        );
        assert_eq!(
            DatabaseBackend::Sqlite.render_pagination(Some(2), 5),
            " LIMIT 2 OFFSET 5"
        );
        assert_eq!(DatabaseBackend::Sqlite.render_pagination(None, 0), "");
    }

    /// Drive a scan over in-memory rows the way repository reads do.
    fn scan(
        visible: &[bool],
        skip: i64,
        take: Option<i64>,
        budget: Option<i64>,
    ) -> crate::Result<(Vec<usize>, i64, usize)> {
        scan_with_overflow(visible, skip, take, budget, None)
    }

    fn scan_with_overflow(
        visible: &[bool],
        skip: i64,
        take: Option<i64>,
        budget: Option<i64>,
        overflow: Option<i64>,
    ) -> crate::Result<(Vec<usize>, i64, usize)> {
        let mut scan = VisibleRowScan::new(skip, take, budget).with_overflow_after_skip(overflow);
        let mut kept = Vec::new();
        let mut reads = 0;
        while let Some(batch) = scan.next_batch()? {
            reads += 1;
            let start = usize::try_from(batch.offset).unwrap();
            let end = (start + usize::try_from(batch.fetch).unwrap()).min(visible.len());
            let rows = (start.min(end)..end).collect::<Vec<_>>();
            let fetched = rows.len();
            let mut examined = 0;
            for row in rows.into_iter().take(batch.examine) {
                if !scan.wants_more() {
                    break;
                }
                examined += 1;
                scan.count_candidate(true)?;
                if scan.record(visible[row]) {
                    kept.push(row);
                }
            }
            scan.finish_batch(batch, fetched, examined);
        }
        Ok((kept, scan.position(), reads))
    }

    #[test]
    fn an_unlimited_read_keeps_rows_after_the_skip_in_a_short_batch() {
        // hidden, hidden, hidden, visible, visible: batches of 1, 2 and 4.
        let rows = [false, false, false, true, true];
        let (kept, position, _) = scan(&rows, 1, None, None).unwrap();
        assert_eq!(kept, vec![4]);
        assert_eq!(position, 5);
    }

    #[test]
    fn one_budget_covers_an_unlimited_read() {
        let rows = [true, false, false, true, false, false, true];
        // Every row fits: all visible rows after the skip.
        assert_eq!(scan(&rows, 1, None, Some(7)).unwrap().0, vec![3, 6]);
        // More rows than the budget remain: refused, not truncated.
        assert!(scan(&rows, 1, None, Some(6)).is_err());
        // A zero budget looks ahead once, then refuses.
        assert!(scan(&rows, 0, None, Some(0)).is_err());
        assert!(scan(&[], 0, None, Some(0)).unwrap().0.is_empty());
        // The fail-closed maximum counts rows after the skip before row
        // policy, and whichever bound is reached first refuses.
        assert_eq!(
            scan_with_overflow(&rows, 1, None, Some(7), Some(6))
                .unwrap()
                .0,
            vec![3, 6]
        );
        assert!(scan_with_overflow(&rows, 1, None, Some(7), Some(5)).is_err());
        assert!(scan_with_overflow(&rows, 1, None, None, Some(5)).is_err());
    }

    #[test]
    fn an_empty_page_reads_nothing() {
        let (kept, _, reads) = scan(&[false, true, true], 5, Some(0), Some(0)).unwrap();
        assert!(kept.is_empty());
        assert_eq!(reads, 0);
    }

    #[test]
    fn a_zero_budget_refuses_only_when_rows_exist() {
        let (kept, _, reads) = scan(&[], 0, Some(3), Some(0)).unwrap();
        assert!(kept.is_empty());
        assert_eq!(reads, 1);
        assert!(scan(&[true], 0, Some(3), Some(0)).is_err());
    }

    #[test]
    fn the_budget_bounds_candidate_rows_and_looks_ahead_once() {
        let rows = [true, false, false, true, false, false, true];
        // Two visible rows need four candidates.
        assert_eq!(scan(&rows, 0, Some(2), Some(4)).unwrap().0, vec![0, 3]);
        assert!(scan(&rows, 0, Some(2), Some(3)).is_err());
        // A budget that covers every row ends with a short look-ahead.
        assert_eq!(scan(&rows, 0, Some(9), Some(7)).unwrap().0, vec![0, 3, 6]);
        assert!(scan(&rows, 0, Some(9), Some(6)).is_err());
    }
}
