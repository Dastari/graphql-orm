//! Request-local database visibility and bounded residual authorization scans.
use super::*;
use crate::db::Database;
use crate::graphql::errors::{OrmErrorCode, OrmPublicError};
use crate::graphql::pagination::*;
use chacha20poly1305::{
    XChaCha20Poly1305,
    aead::{Aead, AeadCore, KeyInit, OsRng},
};
use std::any::TypeId;

fn invalid() -> async_graphql::Error {
    OrmPublicError::new(OrmErrorCode::AuthorizationMisconfigured).into_graphql_error()
}
fn input_error() -> async_graphql::Error {
    OrmPublicError::new(OrmErrorCode::InvalidInput).into_graphql_error()
}
fn query_error(error: sqlx::Error) -> async_graphql::Error {
    OrmPublicError::from(error).into_graphql_error()
}

/// Associates a typed generated filter with its entity. Implementors must render
/// parameterized expressions for that entity and accurately report residual work.
pub trait ReadVisibilityFilter: DatabaseFilter {
    fn entity_type_id() -> TypeId;
    fn backend() -> DatabaseBackend;
}

/// Entity- and backend-bound authorization predicate, built from the same typed
/// filters used by ordinary queries (including supported relation predicates).
/// An empty filter is rejected: use `ReadVisibility::Unrestricted` deliberately.
#[derive(Clone, Debug)]
pub struct ReadPredicate {
    entity: TypeId,
    backend: DatabaseBackend,
    expression: FilterExpression,
}
impl ReadPredicate {
    /// Reject filters requiring residual Rust matching on this backend. A partial
    /// policy must supply a SQL-only prefilter and retain its row callback.
    pub fn from_filter<B: OrmBackend, F: ReadVisibilityFilter>(
        filter: &F,
    ) -> async_graphql::Result<Self> {
        if F::backend() != B::DIALECT || filter.requires_in_memory_filtering(B::DIALECT) {
            return Err(invalid());
        }
        let expression = filter.to_filter_expression().ok_or_else(invalid)?;
        Ok(Self {
            entity: F::entity_type_id(),
            backend: B::DIALECT,
            expression,
        })
    }

    /// A parameterized EXISTS join over validated persisted columns. `keys`
    /// maps source columns to target columns; composite keys are supported.
    /// Target restrictions come from a typed target predicate. This is an
    /// explicit equality relation, not a named relation resolver: include any
    /// discriminator constraints in source/target predicates yourself. Target
    /// entity/row policies are not implicit grants; the policy owns this join.
    pub fn related<B, S, T>(keys: &[(&str, &str)], target: Self) -> async_graphql::Result<Self>
    where
        B: OrmBackend,
        S: Entity + 'static,
        T: Entity + 'static,
    {
        if keys.is_empty() || target.entity != TypeId::of::<T>() || target.backend != B::DIALECT {
            return Err(invalid());
        }
        let mut joins = Vec::new();
        for (source, destination) in keys {
            if !S::columns().iter().any(|c| c.name == *source)
                || !T::columns().iter().any(|c| c.name == *destination)
            {
                return Err(invalid());
            }
            joins.push(format!(
                "__gom_visibility.{} = {}.{}",
                B::DIALECT.quote_identifier(destination),
                B::DIALECT.quote_identifier_path(S::TABLE_NAME),
                B::DIALECT.quote_identifier(source)
            ));
        }
        let mut values = Vec::new();
        let predicate = render_filter_expression(
            DatabaseBackend::Sqlite,
            &target.expression,
            &mut 1,
            &mut values,
        );
        let clause = format!(
            "EXISTS (SELECT 1 FROM (SELECT * FROM {} WHERE {}) AS __gom_visibility WHERE {})",
            B::DIALECT.quote_identifier_path(T::TABLE_NAME),
            predicate,
            joins.join(" AND ")
        );
        Ok(Self {
            entity: TypeId::of::<S>(),
            backend: B::DIALECT,
            expression: FilterExpression::trusted_fragment(clause, values),
        })
    }

    pub fn and(self, other: Self) -> async_graphql::Result<Self> {
        self.combine(other, true)
    }
    pub fn or(self, other: Self) -> async_graphql::Result<Self> {
        self.combine(other, false)
    }
    fn combine(self, other: Self, and: bool) -> async_graphql::Result<Self> {
        if self.entity != other.entity || self.backend != other.backend {
            return Err(invalid());
        }
        let expressions = vec![self.expression, other.expression];
        Ok(Self {
            entity: self.entity,
            backend: self.backend,
            expression: if and {
                FilterExpression::And(expressions)
            } else {
                FilterExpression::Or(expressions)
            },
        })
    }
}

/// The policy's explicit read decision for one request/entity/surface.
#[derive(Clone, Debug)]
pub enum ReadVisibility {
    /// No SQL authorization is available. Every candidate needs a row callback.
    CallbackOnly,
    /// SQL narrows candidates; every remaining candidate still needs a callback.
    Prefilter(ReadPredicate),
    /// SQL is sufficient authorization; row callbacks cannot additionally veto.
    Complete(ReadPredicate),
    /// Explicitly authorize every matching row for this request and entity.
    Unrestricted,
}
impl ReadVisibility {
    pub fn requires_residual_checks(&self) -> bool {
        matches!(self, Self::CallbackOnly | Self::Prefilter(_))
    }
    pub(crate) fn validate<T: Entity + 'static, B: OrmBackend>(&self) -> async_graphql::Result<()> {
        if let Self::Prefilter(p) | Self::Complete(p) = self
            && (p.entity != TypeId::of::<T>() || p.backend != B::DIALECT)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

/// A generated stable keyset order, including its unique final primary key.
/// The values must match the order columns and preserve their scalar types.
pub trait AuthorizedKeysetEntity: Entity {
    const KEYSET_ORDER: &'static [KeysetOrderColumn];
    const KEYSET_FINGERPRINT: &'static str;
    fn keyset_values(&self) -> Vec<KeysetValue>;
}

/// Diagnostics for actual fetched database rows, before entity decoding.
/// SQL contains placeholders; parameter values are deliberately omitted.
pub trait ReadQueryObserver: Send + Sync {
    fn on_read(&self, sql: &str, fetched_rows: usize);
}

/// Server-owned scan bounds and cursor encryption key. Scan endpoints are
/// disabled until installed on a Database. Debug output redacts the key.
#[derive(Clone)]
pub struct AuthorizedScanConfig {
    pub batch_size: u32,
    pub max_scanned_rows: u32,
    key: [u8; 32],
}
impl std::fmt::Debug for AuthorizedScanConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthorizedScanConfig")
            .field("batch_size", &self.batch_size)
            .field("max_scanned_rows", &self.max_scanned_rows)
            .finish_non_exhaustive()
    }
}
impl AuthorizedScanConfig {
    /// Use a secret randomly generated 32-byte key, shared by server instances
    /// that need to accept one another's cursors. Rotating it invalidates cursors.
    pub fn new(key: [u8; 32], batch_size: u32, max_scanned_rows: u32) -> Self {
        Self {
            key,
            batch_size,
            max_scanned_rows,
        }
    }
    fn encode<T: AuthorizedKeysetEntity>(
        &self,
        values: Vec<KeysetValue>,
    ) -> async_graphql::Result<String> {
        let cipher = XChaCha20Poly1305::new((&self.key).into());
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let plaintext = encode_keyset_cursor(
            &format!("scan:{}:{}", T::entity_name(), T::KEYSET_FINGERPRINT),
            values,
        );
        let ciphertext = cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|_| invalid())?;
        let bytes: Vec<u8> = nonce.iter().chain(ciphertext.iter()).copied().collect();
        Ok(format!(
            "goms1.{}",
            bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
        ))
    }
    fn decode<T: AuthorizedKeysetEntity>(
        &self,
        cursor: &str,
    ) -> async_graphql::Result<Vec<KeysetValue>> {
        let error = || OrmPublicError::new(OrmErrorCode::CursorInvalid).into_graphql_error();
        let text = cursor.strip_prefix("goms1.").ok_or_else(error)?;
        if text.len() > 32768
            || !text.len().is_multiple_of(2)
            || !text.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(error());
        }
        let bytes = (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|_| error()))
            .collect::<Result<Vec<_>, _>>()?;
        if bytes.len() < 40 {
            return Err(error());
        }
        let cipher = XChaCha20Poly1305::new((&self.key).into());
        let plaintext = cipher
            .decrypt(bytes[..24].into(), &bytes[24..])
            .map_err(|_| error())?;
        let plaintext = std::str::from_utf8(&plaintext).map_err(|_| error())?;
        decode_keyset_cursor(
            plaintext,
            &format!("scan:{}:{}", T::entity_name(), T::KEYSET_FINGERPRINT),
            T::KEYSET_ORDER.len(),
        )
        .map_err(|e| e.into_graphql_error())
    }
}

/// Forward scan request. Continuations represent underlying scan positions,
/// never visible offsets. Exact totals are available only for complete policies.
#[derive(Clone, Debug, Default, async_graphql::InputObject)]
pub struct AuthorizedScanInput {
    pub after: Option<String>,
    pub limit: Option<i64>,
    #[graphql(default)]
    pub include_total_count: bool,
}

/// Why a scan stopped. Only Exhausted establishes that no further candidate
/// exists. The other states make no claim that another authorized row exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, async_graphql::Enum)]
pub enum ScanStatus {
    Exhausted,
    PageFull,
    BudgetExhausted,
}

#[derive(Clone, Debug, async_graphql::SimpleObject)]
#[graphql(shareable)]
pub struct ScanPageInfo {
    pub status: ScanStatus,
    /// Resume scanning even if this page returned zero visible rows.
    pub continuation: Option<String>,
    /// Absent unless explicitly requested with fully database-visible policy.
    pub total_count: Option<i64>,
}
#[derive(Clone, Debug)]
pub struct AuthorizedScanPage<T> {
    pub nodes: Vec<T>,
    pub page_info: ScanPageInfo,
}

impl<T, B> EntityQuery<T, B>
where
    B: OrmBackend,
    T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    /// AND an already resolved policy with caller filters. Does not enforce
    /// entity/operation access or execute residual callbacks by itself.
    pub fn with_read_visibility(
        mut self,
        visibility: &ReadVisibility,
    ) -> async_graphql::Result<Self> {
        visibility.validate::<T, B>()?;
        if let ReadVisibility::Complete(p) | ReadVisibility::Prefilter(p) = visibility {
            let mut values = Vec::new();
            // EntityQuery stores neutral placeholders; final rendering numbers
            // them together with the caller's filters and ordering parameters.
            let clause = render_filter_expression(
                DatabaseBackend::Sqlite,
                &p.expression,
                &mut 1,
                &mut values,
            );
            self = self.where_values(&format!("({clause})"), values);
        }
        Ok(self)
    }

    async fn authorize_pagination(
        &self,
        db: &Database<B>,
        ctx: Option<&async_graphql::Context<'_>>,
    ) -> async_graphql::Result<ReadVisibility> {
        let surface = if ctx.is_some() {
            EntityAccessSurface::GraphqlQuery
        } else {
            EntityAccessSurface::Repository
        };
        db.ensure_entity_access(
            ctx,
            T::entity_name(),
            T::metadata().read_policy,
            EntityAccessKind::Read,
            surface,
        )
        .await?;
        db.read_visibility::<T>(ctx, surface).await
    }

    async fn authorize_page_fields(
        db: &Database<B>,
        ctx: Option<&async_graphql::Context<'_>>,
        rows: &[T],
    ) -> async_graphql::Result<()> {
        // GraphQL field resolvers retain their existing selection-aware checks.
        if ctx.is_none() {
            for row in rows {
                for field in T::repository_field_policies() {
                    db.ensure_repository_readable_field(
                        None,
                        T::entity_name(),
                        field.api_name,
                        field.read_policy,
                        Some(row),
                    )
                    .await
                    .map_err(|e| e.into_graphql_error())?;
                }
            }
        }
        Ok(())
    }
}

impl<T, B> EntityQuery<T, B>
where
    B: OrmBackend,
    T: AuthorizedKeysetEntity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    fn keyset_bound(
        mut self,
        values: &[KeysetValue],
        before: bool,
        inclusive: bool,
    ) -> async_graphql::Result<Self> {
        let (mut clause, mut binds) = if before {
            render_keyset_before(B::DIALECT, T::KEYSET_ORDER, values, self.values.len() + 1)
        } else {
            render_keyset_after(B::DIALECT, T::KEYSET_ORDER, values, self.values.len() + 1)
        }
        .map_err(|e| e.into_graphql_error())?;
        if inclusive {
            let mut equal = Vec::new();
            for (column, value) in T::KEYSET_ORDER.iter().zip(values) {
                if matches!(value, KeysetValue::Null) {
                    equal.push(format!("{} IS NULL", column.column));
                } else {
                    equal.push(format!(
                        "{} = {}",
                        column.column,
                        B::DIALECT.placeholder(self.values.len() + binds.len() + 1)
                    ));
                    binds.push(super::keyset_sql_value(value).ok_or_else(input_error)?);
                }
            }
            clause = format!("({clause}) OR ({})", equal.join(" AND "));
        }
        self = self.where_values(&clause, binds);
        Ok(self)
    }
    async fn bounded_rows(
        &self,
        db: &Database<B>,
        auth: Option<&DbAuthContext>,
        limit: i64,
    ) -> async_graphql::Result<Vec<T>> {
        self.clone()
            .paginate(&PageInput {
                limit: Some(limit),
                offset: Some(0),
            })
            .fetch_all_with_auth_and_pagination(db, auth, PaginationConfig::unbounded())
            .await
            .map_err(query_error)
    }

    async fn check_repository_candidates(
        db: &Database<B>,
        visibility: &ReadVisibility,
        rows: &[T],
    ) -> async_graphql::Result<()> {
        if visibility.requires_residual_checks() {
            for row in rows {
                if !db
                    .can_read_row(
                        None,
                        T::entity_name(),
                        T::metadata().read_policy,
                        EntityAccessSurface::Repository,
                        row,
                    )
                    .await?
                {
                    return Err(OrmPublicError::forbidden().into_graphql_error());
                }
            }
        }
        Ok(())
    }

    /// A forward keyset connection authorized before LIMIT, count, and probes.
    /// Residual policies must use `fetch_authorized_scan` or legacy exact offsets.
    pub async fn fetch_authorized_keyset(
        self,
        db: &Database<B>,
        ctx: Option<&async_graphql::Context<'_>>,
        auth: Option<&DbAuthContext>,
        page: KeysetPageInput,
    ) -> async_graphql::Result<Connection<T>> {
        self.fetch_authorized_keyset_connection(
            db,
            ctx,
            auth,
            KeysetConnectionInput {
                after: page.after,
                first: page.limit,
                include_total_count: page.include_total_count,
                ..Default::default()
            },
        )
        .await
    }

    /// Bidirectional version of `fetch_authorized_keyset`, with authorized probes
    /// in both directions and counts disabled unless explicitly requested.
    pub async fn fetch_authorized_keyset_connection(
        self,
        db: &Database<B>,
        ctx: Option<&async_graphql::Context<'_>>,
        auth: Option<&DbAuthContext>,
        page: KeysetConnectionInput,
    ) -> async_graphql::Result<Connection<T>> {
        let page = page
            .validate(PaginationConfig::DEFAULT_LIMIT, i64::MAX)
            .map_err(|e| e.into_graphql_error())?;
        let backward = page.direction == KeysetWindowDirection::Backward;
        let visibility = self.authorize_pagination(db, ctx).await?;
        if visibility.requires_residual_checks() && (ctx.is_some() || page.include_total_count) {
            return Err(invalid());
        }
        if self.requires_in_memory_filtering() {
            return Err(input_error());
        }
        let mut base = self.with_read_visibility(&visibility)?;
        base.page = None;
        base.order_clauses = T::KEYSET_ORDER.iter().map(|c| c.order_sql()).collect();
        base.order_values.clear();
        let limit = db
            .pagination_config()
            .resolve_page(
                Some(&PageInput {
                    limit: Some(page.limit),
                    offset: Some(0),
                }),
                true,
            )
            .limit
            .unwrap_or(PaginationConfig::DEFAULT_LIMIT)
            .max(1);
        let after = page
            .cursor
            .as_deref()
            .map(|cursor| {
                decode_keyset_cursor(cursor, T::KEYSET_FINGERPRINT, T::KEYSET_ORDER.len())
                    .map_err(|e| e.into_graphql_error())
            })
            .transpose()?;
        let total_count = if page.include_total_count {
            Some(base.count_with_auth(db, auth).await.map_err(query_error)?)
        } else {
            None
        };
        let mut query = if let Some(values) = &after {
            base.clone().keyset_bound(values, backward, false)?
        } else {
            base.clone()
        };
        if backward {
            query.order_clauses = T::KEYSET_ORDER
                .iter()
                .map(|c| c.reversed().order_sql())
                .collect();
        }
        let mut nodes = query
            .bounded_rows(db, auth, limit.saturating_add(1))
            .await?;
        Self::check_repository_candidates(db, &visibility, &nodes).await?;
        let has_more = nodes.len() > limit as usize;
        nodes.truncate(limit as usize);
        let opposite = if let Some(values) = nodes.first().map(T::keyset_values).or(after) {
            let probe = base
                .clone()
                .keyset_bound(&values, !backward, nodes.is_empty())?
                .bounded_rows(db, auth, 1)
                .await?;
            Self::check_repository_candidates(db, &visibility, &probe).await?;
            !probe.is_empty()
        } else {
            false
        };
        if backward {
            nodes.reverse();
        }
        let (has_next_page, has_previous_page) = if backward {
            (opposite, has_more)
        } else {
            (has_more, opposite)
        };
        Self::authorize_page_fields(db, ctx, &nodes).await?;
        let edges = nodes
            .into_iter()
            .map(|node| Edge {
                cursor: encode_keyset_cursor(T::KEYSET_FINGERPRINT, node.keyset_values()),
                node,
            })
            .collect::<Vec<_>>();
        Ok(Connection {
            page_info: PageInfo {
                has_next_page,
                has_previous_page,
                start_cursor: edges.first().map(|e| e.cursor.clone()),
                end_cursor: edges.last().map(|e| e.cursor.clone()),
                total_count,
            },
            edges,
        })
    }

    /// Scan bounded database batches, advancing past every examined candidate.
    /// Empty visible batches do not establish exhaustion. No exact callback
    /// count is attempted; requesting one fails before fetching rows.
    pub async fn fetch_authorized_scan(
        self,
        db: &Database<B>,
        ctx: Option<&async_graphql::Context<'_>>,
        auth: Option<&DbAuthContext>,
        page: AuthorizedScanInput,
    ) -> async_graphql::Result<AuthorizedScanPage<T>> {
        let visibility = self.authorize_pagination(db, ctx).await?;
        let config = db.authorized_scan_config().ok_or_else(invalid)?;
        if config.batch_size == 0 || config.max_scanned_rows == 0 {
            return Err(invalid());
        }
        if self.requires_in_memory_filtering() || page.limit.is_some_and(|n| n <= 0) {
            return Err(input_error());
        }
        if page.include_total_count && visibility.requires_residual_checks() {
            return Err(input_error());
        }
        let mut base = self.with_read_visibility(&visibility)?;
        base.page = None;
        base.order_clauses = T::KEYSET_ORDER.iter().map(|c| c.order_sql()).collect();
        base.order_values.clear();
        let limit = db
            .pagination_config()
            .resolve_page(
                Some(&PageInput {
                    limit: page.limit,
                    offset: Some(0),
                }),
                true,
            )
            .limit
            .unwrap_or(PaginationConfig::DEFAULT_LIMIT)
            .max(1) as usize;
        let mut position = page
            .after
            .as_deref()
            .map(|c| config.decode::<T>(c))
            .transpose()?;
        let total_count = if page.include_total_count {
            Some(base.count_with_auth(db, auth).await.map_err(query_error)?)
        } else {
            None
        };
        let mut nodes = Vec::new();
        let mut scanned = 0;
        let status = 'scan: loop {
            let batch_size = config
                .batch_size
                .min(config.max_scanned_rows - scanned)
                .min((limit - nodes.len()).min(u32::MAX as usize) as u32);
            let query = if let Some(values) = &position {
                base.clone().keyset_bound(values, false, false)?
            } else {
                base.clone()
            };
            let rows = query.bounded_rows(db, auth, i64::from(batch_size)).await?;
            let short = rows.len() < batch_size as usize;
            for row in rows {
                position = Some(row.keyset_values());
                scanned += 1;
                if !visibility.requires_residual_checks()
                    || db
                        .can_read_row(
                            ctx,
                            T::entity_name(),
                            T::metadata().read_policy,
                            if ctx.is_some() {
                                EntityAccessSurface::GraphqlQuery
                            } else {
                                EntityAccessSurface::Repository
                            },
                            &row,
                        )
                        .await?
                {
                    nodes.push(row);
                }
            }
            if short {
                break 'scan ScanStatus::Exhausted;
            }
            if nodes.len() == limit {
                break 'scan ScanStatus::PageFull;
            }
            if scanned == config.max_scanned_rows {
                break 'scan ScanStatus::BudgetExhausted;
            }
        };
        Self::authorize_page_fields(db, ctx, &nodes).await?;
        let continuation = if status == ScanStatus::Exhausted {
            None
        } else {
            Some(config.encode::<T>(position.ok_or_else(invalid)?)?)
        };
        Ok(AuthorizedScanPage {
            nodes,
            page_info: ScanPageInfo {
                status,
                continuation,
                total_count,
            },
        })
    }
}
