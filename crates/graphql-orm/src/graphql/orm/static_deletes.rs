//! Explicit, bounded GraphQL dependent deletes for generated static entities.
use super::*;
use crate::async_graphql::{Context, Error, Result};
use futures::future::BoxFuture;
use std::{
    any::{Any, TypeId},
    collections::{BTreeMap, BTreeSet},
    marker::PhantomData,
    sync::Arc,
};

fn refusal() -> Error {
    Error::new("Dependent delete refused")
}

/// Backend capability for pinned physical dependency discovery.
/// Only SQLite and PostgreSQL implement this capability.
#[doc(hidden)]
pub trait StaticDeleteBackend: TransactionBackend {
    /// Refuse physical relation behavior that can bypass the planned writes.
    /// This is also included in full catalogue revalidation before every write.
    /// SQLite has no inheritance, partitions, rules or foreign-table equivalent.
    fn validate_delete_relations<'a>(
        _tx: &'a mut Self::Transaction<'_>,
        _ownership: &'a ManagedTableSet,
    ) -> BoxFuture<'a, crate::Result<()>> {
        Box::pin(async { Ok(()) })
    }
    /// Read and fully validate the catalogue on the pinned transaction.
    /// Repeat even checks not represented in the returned model on every call.
    /// The caller requires structural equality with the planning snapshot;
    /// no rows are rediscovered and no changed dependency graph is accepted.
    fn dependencies<'a>(
        tx: &'a mut Self::Transaction<'_>,
        ownership: &'a ManagedTableSet,
    ) -> BoxFuture<'a, crate::Result<OwnedSchemaModel>>;
}
#[cfg(feature = "sqlite")]
fn sqlite_affinity(declared: &str, strict: bool) -> &'static str {
    let declared = declared.to_ascii_uppercase();
    // SQLite changes STRICT ANY to BLOB affinity instead of NUMERIC.
    if strict && declared.trim() == "ANY" {
        "BLOB"
    } else if declared.contains("INT") {
        "INTEGER"
    } else if ["CHAR", "CLOB", "TEXT"]
        .iter()
        .any(|kind| declared.contains(kind))
    {
        "TEXT"
    } else if declared.is_empty() || declared.contains("BLOB") {
        "BLOB"
    } else if ["REAL", "FLOA", "DOUB"]
        .iter()
        .any(|kind| declared.contains(kind))
    {
        "REAL"
    } else {
        "NUMERIC"
    }
}
#[cfg(feature = "sqlite")]
impl StaticDeleteBackend for SqliteBackend {
    fn dependencies<'a>(
        tx: &'a mut Self::Transaction<'_>,
        ownership: &'a ManagedTableSet,
    ) -> BoxFuture<'a, crate::Result<OwnedSchemaModel>> {
        Box::pin(async move {
            let temporary: Vec<String> =
                sqlx::query_scalar("SELECT name FROM sqlite_temp_master WHERE type='table'")
                    .fetch_all(tx.as_mut())
                    .await?;
            if temporary.iter().any(|name| {
                ownership
                    .tables()
                    .any(|owned| owned.eq_ignore_ascii_case(name))
            }) {
                return Err(sqlx::Error::Protocol("Dependent delete refused".into()));
            }
            // A database trigger can introduce referencers inside DELETE,
            // after the ORM's last check and before the physical FK action.
            let triggered:Vec<String>=sqlx::query_scalar("SELECT tbl_name FROM sqlite_master WHERE type='trigger' UNION ALL SELECT tbl_name FROM sqlite_temp_master WHERE type='trigger'").fetch_all(tx.as_mut()).await?;
            if triggered.iter().any(|name| {
                ownership
                    .tables()
                    .any(|owned| owned.eq_ignore_ascii_case(name))
            }) {
                return Err(sqlx::Error::Protocol("Dependent delete refused".into()));
            }
            super::owned_foreign_keys::validate_sqlite(tx.as_mut(), ownership)
                .await
                .map_err(|_| sqlx::Error::Protocol("Dependent delete refused".into()))?;
            let physical = super::migrations::introspect_owned_sqlite_connection_with_internal(
                tx.as_mut(),
                true,
            )
            .await?;
            let mut affinities = BTreeMap::new();
            for table in ownership.tables() {
                let ddl: Option<String> = sqlx::query_scalar(
                    "SELECT sql FROM sqlite_master WHERE type='table' AND name=?",
                )
                .bind(table)
                .fetch_optional(tx.as_mut())
                .await?
                .flatten();
                let Some(ddl) = ddl else {
                    return Err(sqlx::Error::Protocol("Dependent delete refused".into()));
                };
                // table_xinfo omits collations. Conservatively refuse any
                // explicit COLLATE token in the table SQL, including quoted
                // identifiers/comments: parsing a subset must never miss one.
                if ddl
                    .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .any(|token| token.eq_ignore_ascii_case("COLLATE"))
                {
                    return Err(sqlx::Error::Protocol("Dependent delete refused".into()));
                }
                let strict: i64 = sqlx::query_scalar(
                    "SELECT strict FROM pragma_table_list() WHERE schema='main' AND name=?",
                )
                .bind(table)
                .fetch_one(tx.as_mut())
                .await?;
                let columns: Vec<(String, String)> =
                    sqlx::query_as("SELECT name, type FROM pragma_table_xinfo(?)")
                        .bind(table)
                        .fetch_all(tx.as_mut())
                        .await?;
                for (column, declared) in columns {
                    affinities.insert(
                        (table.to_owned(), column),
                        sqlite_affinity(&declared, strict != 0),
                    );
                }
            }
            for source in &physical.tables {
                for fk in &source.foreign_keys {
                    if ownership.tables().any(|target| target == fk.target_table) {
                        for pair in &fk.column_pairs {
                            let parent = affinities
                                .get(&(fk.target_table.clone(), pair.target_column.clone()));
                            let child = affinities
                                .get(&(source.table_name.clone(), pair.source_column.clone()));
                            if parent.is_none() || child.is_none() || parent != child {
                                return Err(sqlx::Error::Protocol(
                                    "Dependent delete refused".into(),
                                ));
                            }
                        }
                    }
                }
            }
            Ok(physical)
        })
    }
}
// The PostgreSQL gate runs inside the caller's write transaction. Its catalog
// reads are pure: every catalog relation, function and operator is named with
// its schema and nothing changes session or transaction state, so the reads
// cannot alter the caller's transaction, including when the future is dropped
// part way. No nested transaction is opened here.
#[cfg(feature = "postgres")]
impl StaticDeleteBackend for PostgresBackend {
    fn validate_delete_relations<'a>(
        tx: &'a mut Self::Transaction<'_>,
        ownership: &'a ManagedTableSet,
    ) -> BoxFuture<'a, crate::Result<()>> {
        Box::pin(async move {
            // DELETE also visits inheritance/partition descendants, and rules
            // can execute unrelated writes. Only ordinary standalone heap
            // tables have the dependency semantics modeled by this planner.
            postgres_unsupported_delete_relations(tx.as_mut(), ownership).await
        })
    }
    fn dependencies<'a>(
        tx: &'a mut Self::Transaction<'_>,
        ownership: &'a ManagedTableSet,
    ) -> BoxFuture<'a, crate::Result<OwnedSchemaModel>> {
        Box::pin(async move {
            // Acquire deterministic schema/write fences before the first MVCC
            // snapshot (including auth-setting SELECTs). Incoming ADD/DROP FK
            // operations need conflicting relation locks on these targets.
            let tables = ownership
                .tables()
                .map(|table| DatabaseBackend::Postgres.quote_identifier_path(table))
                .collect::<Vec<_>>()
                .join(", ");
            sqlx::query(&format!("LOCK TABLE {tables} IN SHARE ROW EXCLUSIVE MODE"))
                .execute(tx.as_mut())
                .await?;
            for table in ownership.tables() {
                // Each name must resolve, under the session's search_path, to
                // the table of that name in the current schema. The query uses
                // only schema-qualified functions and no operators.
                let (bare, qualified): (
                    Option<sqlx::postgres::types::Oid>,
                    Option<sqlx::postgres::types::Oid>,
                ) = sqlx::query_as(
                    "SELECT pg_catalog.to_regclass(pg_catalog.quote_ident($1))::pg_catalog.oid,
                            pg_catalog.to_regclass(pg_catalog.textcat(pg_catalog.textcat(
                                pg_catalog.quote_ident(pg_catalog.current_schema()::pg_catalog.text), '.'),
                                pg_catalog.quote_ident($1)))::pg_catalog.oid",
                )
                .bind(table)
                .fetch_one(tx.as_mut())
                .await?;
                if bare.is_none() || bare != qualified {
                    return Err(sqlx::Error::Protocol("Dependent delete refused".into()));
                }
            }
            Self::validate_delete_relations(tx, ownership).await?;
            // Turning row_security off does not bypass RLS: PostgreSQL errors
            // rather than silently hiding dependents for a non-bypass role.
            sqlx::query("SET LOCAL row_security = off")
                .execute(tx.as_mut())
                .await?;
            postgres_dependency_reads(tx.as_mut(), ownership).await
        })
    }
}

/// Refuse when any owned table is not an ordinary standalone heap table
/// without rules. A pure, fully qualified catalog read.
#[cfg(feature = "postgres")]
fn postgres_unsupported_delete_relations<'a>(
    conn: &'a mut sqlx::PgConnection,
    ownership: &'a ManagedTableSet,
) -> BoxFuture<'a, crate::Result<()>> {
    Box::pin(async move {
        let unsupported: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM pg_catalog.pg_class c
                JOIN pg_catalog.pg_namespace n ON n.oid OPERATOR(pg_catalog.=) c.relnamespace
                WHERE n.nspname OPERATOR(pg_catalog.=) pg_catalog.current_schema()
                  AND c.relname::pg_catalog.text OPERATOR(pg_catalog.=) ANY ($1::pg_catalog.text[])
                  AND (c.relkind OPERATOR(pg_catalog.<>) 'r'::pg_catalog.\"char\" OR c.relispartition
                       OR EXISTS(SELECT 1 FROM pg_catalog.pg_inherits i
                                 WHERE i.inhparent OPERATOR(pg_catalog.=) c.oid OR i.inhrelid OPERATOR(pg_catalog.=) c.oid)
                       OR EXISTS(SELECT 1 FROM pg_catalog.pg_rewrite r
                                 WHERE r.ev_class OPERATOR(pg_catalog.=) c.oid
                                   AND r.rulename OPERATOR(pg_catalog.<>) '_RETURN'::pg_catalog.name))
            )",
        )
        .bind(ownership.tables().collect::<Vec<_>>())
        .fetch_one(&mut *conn)
        .await?;
        if unsupported {
            return Err(sqlx::Error::Protocol("Dependent delete refused".into()));
        }
        Ok(())
    })
}

/// The dependent-delete gate's catalog reads and the physical model it plans
/// against. Pure, fully qualified catalog reads on the caller's connection;
/// the physical model's types and defaults render with the session's path,
/// as the planner compares them.
#[cfg(feature = "postgres")]
pub(super) fn postgres_dependency_reads<'a>(
    conn: &'a mut sqlx::PgConnection,
    ownership: &'a ManagedTableSet,
) -> BoxFuture<'a, crate::Result<OwnedSchemaModel>> {
    Box::pin(async move {
        super::owned_foreign_keys::validate_postgres(&mut *conn, ownership)
            .await
            .map_err(|_| sqlx::Error::Protocol("Dependent delete refused".into()))?;
        // Only native FK/unique/exclusion constraint triggers have modeled
        // effects. User and extension trigger bodies are opaque. The ORM's
        // own RLS search-document trigger only deletes from its search
        // table, which holds no foreign keys or entity rows; it is exempt
        // only when it fires normally, has the exact type, arguments and
        // target, and calls the unmodified ORM function. The ORM's change
        // journal trigger only appends to the journal, which likewise holds
        // no foreign keys or entity rows; it is exempt only when it fires
        // normally, has the exact type, and calls the unmodified recorder
        // owned by the table owner.
        let owned_tables = ownership
            .tables()
            .map(|table| table.to_string())
            .collect::<Vec<_>>();
        let owned_search_tables = owned_tables
            .iter()
            .map(|table| super::search::search_table_name(table))
            .collect::<Vec<_>>();
        let triggered: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_catalog.pg_trigger tr
                JOIN pg_catalog.pg_class t ON t.oid OPERATOR(pg_catalog.=) tr.tgrelid
                JOIN pg_catalog.pg_namespace n ON n.oid OPERATOR(pg_catalog.=) t.relnamespace
                JOIN pg_catalog.pg_proc p ON p.oid OPERATOR(pg_catalog.=) tr.tgfoid
                JOIN pg_catalog.pg_namespace pn ON pn.oid OPERATOR(pg_catalog.=) p.pronamespace
                LEFT JOIN pg_catalog.pg_constraint con ON con.oid OPERATOR(pg_catalog.=) tr.tgconstraint
                WHERE n.nspname OPERATOR(pg_catalog.=) pg_catalog.current_schema()
                  AND t.relname::pg_catalog.text OPERATOR(pg_catalog.=) ANY ($1::pg_catalog.text[])
                  AND NOT COALESCE(tr.tgisinternal AND pn.nspname OPERATOR(pg_catalog.=) 'pg_catalog'::pg_catalog.name
                                   AND con.contype OPERATOR(pg_catalog.=) ANY (ARRAY['f', 'u', 'x']::pg_catalog.\"char\"[]), false)
                  AND NOT (NOT tr.tgisinternal AND tr.tgname::pg_catalog.text OPERATOR(pg_catalog.=) $2::pg_catalog.text
                           AND tr.tgenabled OPERATOR(pg_catalog.=) 'O'::pg_catalog.\"char\"
                           AND tr.tgtype OPERATOR(pg_catalog.=) $4::pg_catalog.int2
                           AND tr.tgnargs OPERATOR(pg_catalog.=) 4::pg_catalog.int2
                           AND pn.nspname OPERATOR(pg_catalog.=) 'graphql_orm'::pg_catalog.name
                           AND p.proname::pg_catalog.text OPERATOR(pg_catalog.=) $3::pg_catalog.text
                           AND p.pronargs OPERATOR(pg_catalog.=) 0::pg_catalog.int2 AND NOT p.prosecdef
                           AND p.prosrc OPERATOR(pg_catalog.=) $5::pg_catalog.text
                           AND p.proconfig OPERATOR(pg_catalog.=) ARRAY[$6]::pg_catalog.text[]
                           AND EXISTS (SELECT 1 FROM ROWS FROM (pg_catalog.unnest($1::pg_catalog.text[]), pg_catalog.unnest($7::pg_catalog.text[])) AS e(base, search)
                                       WHERE e.base OPERATOR(pg_catalog.=) t.relname::pg_catalog.text
                                         AND (pg_catalog.string_to_array(pg_catalog.encode(tr.tgargs, 'escape'), '\\000'))[1] OPERATOR(pg_catalog.=) pg_catalog.current_schema()::pg_catalog.text
                                         AND (pg_catalog.string_to_array(pg_catalog.encode(tr.tgargs, 'escape'), '\\000'))[2] OPERATOR(pg_catalog.=) e.search))
                  AND NOT (NOT tr.tgisinternal AND tr.tgname::pg_catalog.text OPERATOR(pg_catalog.=) $8::pg_catalog.text
                           AND tr.tgenabled OPERATOR(pg_catalog.=) 'O'::pg_catalog.\"char\"
                           AND tr.tgtype OPERATOR(pg_catalog.=) $9::pg_catalog.int2 AND tr.tgqual IS NULL
                           AND tr.tgnargs OPERATOR(pg_catalog.=) 7::pg_catalog.int2
                           AND pn.nspname OPERATOR(pg_catalog.=) 'graphql_orm'::pg_catalog.name
                           AND p.proname::pg_catalog.text OPERATOR(pg_catalog.=) $10::pg_catalog.text
                           AND p.pronargs OPERATOR(pg_catalog.=) 0::pg_catalog.int2 AND p.prosecdef
                           AND p.proowner OPERATOR(pg_catalog.=) t.relowner
                           AND p.prosrc OPERATOR(pg_catalog.=) $11::pg_catalog.text
                           AND p.proconfig OPERATOR(pg_catalog.=) ARRAY[$6]::pg_catalog.text[]))",
        )
        .bind(&owned_tables)
        .bind(super::rls::SEARCH_DELETE_TRIGGER)
        .bind(super::rls::SEARCH_DELETE_FUNCTION)
        .bind(i16::try_from(super::rls::SEARCH_DELETE_TRIGGER_TYPE).unwrap_or(0))
        .bind(super::rls::SEARCH_DELETE_FUNCTION_BODY)
        .bind(super::rls::SEARCH_DELETE_FUNCTION_CONFIG)
        .bind(&owned_search_tables)
        .bind(super::rls::CHANGE_JOURNAL_TRIGGER)
        .bind(i16::try_from(super::rls::CHANGE_JOURNAL_TRIGGER_TYPE).unwrap_or(0))
        .bind(super::rls::CHANGE_JOURNAL_FUNCTION)
        .bind(super::rls::CHANGE_JOURNAL_FUNCTION_BODY)
        .fetch_one(&mut *conn)
        .await?;
        if triggered {
            return Err(sqlx::Error::Protocol("Dependent delete refused".into()));
        }
        // The planner and final no-referencer check use the child
        // column's equality semantics; FK enforcement uses the parent's.
        // Prove them identical before either lookup. Relation locks fence
        // concurrent DDL; hook changes are revalidated before writes. Extension/
        // domain equality and blank-padded char vs bound text are unmodeled.
        let incompatible: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM pg_catalog.pg_constraint con
                JOIN pg_catalog.pg_class t ON t.oid OPERATOR(pg_catalog.=) con.confrelid
                JOIN pg_catalog.pg_namespace n ON n.oid OPERATOR(pg_catalog.=) t.relnamespace
                CROSS JOIN LATERAL ROWS FROM (pg_catalog.unnest(con.conkey), pg_catalog.unnest(con.confkey)) AS pair(child_att, parent_att)
                JOIN pg_catalog.pg_attribute child
                  ON child.attrelid OPERATOR(pg_catalog.=) con.conrelid AND child.attnum OPERATOR(pg_catalog.=) pair.child_att
                JOIN pg_catalog.pg_attribute parent
                  ON parent.attrelid OPERATOR(pg_catalog.=) con.confrelid AND parent.attnum OPERATOR(pg_catalog.=) pair.parent_att
                JOIN pg_catalog.pg_type key_type ON key_type.oid OPERATOR(pg_catalog.=) parent.atttypid
                JOIN pg_catalog.pg_namespace type_ns ON type_ns.oid OPERATOR(pg_catalog.=) key_type.typnamespace
                LEFT JOIN pg_catalog.pg_collation cc ON cc.oid OPERATOR(pg_catalog.=) child.attcollation
                LEFT JOIN pg_catalog.pg_collation pc ON pc.oid OPERATOR(pg_catalog.=) parent.attcollation
                WHERE con.contype OPERATOR(pg_catalog.=) 'f'::pg_catalog.\"char\"
                  AND n.nspname OPERATOR(pg_catalog.=) pg_catalog.current_schema()
                  AND t.relname::pg_catalog.text OPERATOR(pg_catalog.=) ANY ($1::pg_catalog.text[])
                  AND (child.atttypid OPERATOR(pg_catalog.<>) parent.atttypid
                       OR child.atttypmod OPERATOR(pg_catalog.<>) parent.atttypmod
                       OR child.attcollation OPERATOR(pg_catalog.<>) parent.attcollation
                       OR NOT COALESCE(cc.collisdeterministic, true)
                       OR NOT COALESCE(pc.collisdeterministic, true)
                       OR type_ns.nspname OPERATOR(pg_catalog.<>) 'pg_catalog'::pg_catalog.name
                       OR key_type.typname OPERATOR(pg_catalog.=) 'bpchar'::pg_catalog.name)
            )",
        )
        .bind(ownership.tables().collect::<Vec<_>>())
        .fetch_one(&mut *conn)
        .await?;
        if incompatible {
            return Err(sqlx::Error::Protocol("Dependent delete refused".into()));
        }
        let physical =
            super::migrations::introspect_owned_postgres_connection_with_internal(&mut *conn, true)
                .await?;
        // information_schema hides tables without privileges; pg_catalog
        // must prove that no incoming FK source disappeared from that view.
        let visible = physical
            .tables
            .iter()
            .map(|t| t.table_name.as_str())
            .collect::<Vec<_>>();
        let tables = ownership.tables().collect::<Vec<_>>();
        let hidden: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_catalog.pg_constraint con
                JOIN pg_catalog.pg_class s ON s.oid OPERATOR(pg_catalog.=) con.conrelid
                JOIN pg_catalog.pg_namespace sn ON sn.oid OPERATOR(pg_catalog.=) s.relnamespace
                JOIN pg_catalog.pg_class t ON t.oid OPERATOR(pg_catalog.=) con.confrelid
                JOIN pg_catalog.pg_namespace tn ON tn.oid OPERATOR(pg_catalog.=) t.relnamespace
                WHERE con.contype OPERATOR(pg_catalog.=) 'f'::pg_catalog.\"char\"
                  AND tn.nspname OPERATOR(pg_catalog.=) pg_catalog.current_schema()
                  AND t.relname::pg_catalog.text OPERATOR(pg_catalog.=) ANY ($1::pg_catalog.text[])
                  AND (sn.nspname OPERATOR(pg_catalog.<>) pg_catalog.current_schema()
                       OR NOT (s.relname::pg_catalog.text OPERATOR(pg_catalog.=) ANY ($2::pg_catalog.text[]))))",
        )
        .bind(tables)
        .bind(visible)
        .fetch_one(&mut *conn)
        .await?;
        if hidden {
            return Err(sqlx::Error::Protocol("Dependent delete refused".into()));
        }
        Ok(physical)
    })
}

/// Internal typed row retained by a bounded dependent-delete plan.
#[doc(hidden)]
#[derive(Clone)]
pub struct StaticDeleteRow {
    pub key: SqlValue,
    pub entity: Arc<dyn Any + Send + Sync>,
}
/// Captured direct resolver notification, invoked only after the graph commits.
#[doc(hidden)]
pub trait StaticCommittedNotification<B: StaticDeleteBackend>: Send {
    fn notify<'a>(
        self: Box<Self>,
        ctx: &'a Context<'_>,
        db: &'a crate::db::Database<B>,
    ) -> BoxFuture<'a, Result<()>>;
}
/// Optional captured post-commit work from one direct generated write.
#[doc(hidden)]
pub type StaticWriteCompletion<B> = Option<Box<dyn StaticCommittedNotification<B>>>;
/// Direct mutation result retained across a transaction-bound internal step.
/// A dependent rejection is sanitized; a root rejection keeps its direct result.
#[doc(hidden)]
pub enum StaticWriteResult<T> {
    /// The direct operation succeeded or is ready to apply.
    Ready(T),
    /// The direct operation returned a false mutation result and public message.
    Rejected(String),
}
impl<T> StaticWriteResult<T> {
    fn dependent(self) -> Result<T> {
        match self {
            Self::Ready(value) => Ok(value),
            Self::Rejected(_) => Err(refusal()),
        }
    }
}
/// Prepared direct GraphQL write. Authorization and before hooks have finished;
/// applying it rechecks direct GraphQL authority, performs SQL and after hooks,
/// and queues events on the same transaction.
#[doc(hidden)]
pub trait StaticPreparedWrite<B: StaticDeleteBackend>: Send {
    fn apply<'a>(
        self: Box<Self>,
        ctx: &'a Context<'_>,
        tx: &'a mut MutationContext<'_, B>,
    ) -> BoxFuture<'a, Result<StaticWriteResult<StaticWriteCompletion<B>>>>;
}
/// Generated capability, available only on opted-in single-key writable entities.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "dependent_deletes requires `{Self}` to also opt in with #[graphql_entity(dependent_deletes)]"
)]
pub trait StaticDeleteEntity<B: StaticDeleteBackend>:
    Entity + super::DatabaseRls + Send + Sync + 'static
{
    fn select<'a>(
        tx: &'a mut MutationContext<'_, B>,
        column: &'a str,
        value: SqlValue,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<StaticDeleteRow>>>;
    fn prepare<'a>(
        ctx: &'a Context<'_>,
        tx: &'a mut MutationContext<'_, B>,
        row: StaticDeleteRow,
        nulls: Vec<String>,
        expected: Option<i64>,
    ) -> BoxFuture<'a, Result<StaticWriteResult<Box<dyn StaticPreparedWrite<B>>>>>;
}
trait Adapter<B: StaticDeleteBackend>: Send + Sync {
    fn entity_type_id(&self) -> TypeId;
    /// Fail closed when this entity has PostgreSQL RLS and the request has no
    /// database authorization; see [`super::ensure_rls_request_context`].
    fn ensure_rls_context(&self, ctx: &Context<'_>) -> Result<()>;
    fn metadata(&self) -> &'static EntityMetadata;
    fn select<'a>(
        &self,
        tx: &'a mut MutationContext<'_, B>,
        column: &'a str,
        value: SqlValue,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<StaticDeleteRow>>>;
    fn prepare<'a>(
        &self,
        ctx: &'a Context<'_>,
        tx: &'a mut MutationContext<'_, B>,
        row: StaticDeleteRow,
        nulls: Vec<String>,
        expected: Option<i64>,
    ) -> BoxFuture<'a, Result<StaticWriteResult<Box<dyn StaticPreparedWrite<B>>>>>;
}
struct TypedAdapter<T>(PhantomData<T>);
impl<B: StaticDeleteBackend, T: StaticDeleteEntity<B>> Adapter<B> for TypedAdapter<T> {
    fn entity_type_id(&self) -> TypeId {
        TypeId::of::<T>()
    }
    fn ensure_rls_context(&self, ctx: &Context<'_>) -> Result<()> {
        super::ensure_rls_request_context::<T, B>(ctx, T::entity_name())
    }
    fn metadata(&self) -> &'static EntityMetadata {
        T::metadata()
    }
    fn select<'a>(
        &self,
        tx: &'a mut MutationContext<'_, B>,
        column: &'a str,
        value: SqlValue,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<StaticDeleteRow>>> {
        T::select(tx, column, value, limit)
    }
    fn prepare<'a>(
        &self,
        ctx: &'a Context<'_>,
        tx: &'a mut MutationContext<'_, B>,
        row: StaticDeleteRow,
        nulls: Vec<String>,
        expected: Option<i64>,
    ) -> BoxFuture<'a, Result<StaticWriteResult<Box<dyn StaticPreparedWrite<B>>>>> {
        T::prepare(ctx, tx, row, nulls, expected)
    }
}
/// Explicit GraphQL dependent-write registry and bounds for one delete.
///
/// Register every participating entity with [`Self::register`] and attach this
/// value to schema or request data. Each entity must declare
/// `#[graphql_entity(dependent_deletes)]`. The opt-in changes no DDL. Unknown
/// physical dependents, composite foreign keys and cycles refuse the delete.
/// Unregistered opted-in root entities also refuse; there is no physical fallback.
pub struct StaticDependentDeletes<B: StaticDeleteBackend> {
    limits: RuntimeDependentLimits,
    entities: BTreeMap<&'static str, Box<dyn Adapter<B>>>,
}
impl<B: StaticDeleteBackend> StaticDependentDeletes<B> {
    /// Create a registry with explicit row and relation-depth limits.
    pub fn new(limits: RuntimeDependentLimits) -> Self {
        Self {
            limits,
            entities: BTreeMap::new(),
        }
    }
    /// Register one opted-in entity. Duplicate physical table identities refuse.
    ///
    /// # Errors
    /// Returns a sanitized composition refusal for duplicate table identities.
    pub fn register<T: StaticDeleteEntity<B>>(mut self) -> Result<Self> {
        if self
            .entities
            .insert(T::TABLE_NAME, Box::new(TypedAdapter::<T>(PhantomData)))
            .is_some()
        {
            return Err(refusal());
        }
        Ok(self)
    }
    /// Execute one generated GraphQL delete. Events publish only after commit.
    #[doc(hidden)]
    pub async fn delete<T: StaticDeleteEntity<B>>(
        &self,
        ctx: &Context<'_>,
        key: SqlValue,
        expected: Option<i64>,
    ) -> Result<Option<String>> {
        let table = T::TABLE_NAME;
        // A physical table may have multiple GraphQL views. Never substitute
        // another view's policies, assurance, hooks or event type for this root.
        if self
            .entities
            .get(table)
            .is_none_or(|entry| entry.entity_type_id() != TypeId::of::<T>())
        {
            return Err(refusal());
        }
        let (rejection, notifications) = self.delete_inner(ctx, table, key, expected).await?;
        if rejection.is_some() {
            return Ok(rejection);
        }
        let db = ctx.data::<crate::db::Database<B>>()?;
        let mut failure = None;
        for notification in notifications {
            if let Err(error) = notification.notify(ctx, db).await {
                failure.get_or_insert(error);
            }
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(None),
        }
    }
    async fn delete_inner(
        &self,
        ctx: &Context<'_>,
        table: &str,
        key: SqlValue,
        expected: Option<i64>,
    ) -> Result<(Option<String>, Vec<Box<dyn StaticCommittedNotification<B>>>)> {
        let db = ctx.data::<crate::db::Database<B>>()?;
        // Every registered entity that declares a write policy needs its row
        // provider, checked before any dependent is selected or locked.
        for adapter in self.entities.values() {
            let metadata = adapter.metadata();
            db.ensure_row_write_provider(metadata.entity_name, metadata.write_policy)?;
        }
        let auth = ctx.data_opt::<DbAuthContext>();
        let mut tx = MutationContext::<B>::begin(db, TransactionMode::StateMachine)
            .await
            .map_err(crate::graphql::errors::graphql_error_from_sqlx)?;
        let ownership = ManagedTableSet::new(self.entities.keys().map(|s| s.to_string()))
            .map_err(|_| refusal())?;
        let physical = B::dependencies(&mut tx.tx, &ownership)
            .await
            .map_err(|_| refusal())?;
        if !self.entities.keys().all(|name| {
            physical
                .tables
                .iter()
                .any(|table| table.table_name == *name)
        }) {
            return Err(refusal());
        }
        B::apply_auth_context_to_write_transaction(&mut tx.tx, auth)
            .await
            .map_err(|_| refusal())?;
        let root = self.entities.get(table).ok_or_else(refusal)?;
        root.ensure_rls_context(ctx)?;
        let mut rows = root
            .select(&mut tx, root.metadata().primary_key, key, 2)
            .await?;
        if rows.is_empty() {
            return Ok((Some("Entity not found".to_owned()), Vec::new()));
        }
        if rows.len() != 1 {
            return Err(refusal());
        }
        struct Node {
            table: String,
            row: StaticDeleteRow,
            depth: usize,
        }
        let mut nodes = vec![Node {
            table: table.to_owned(),
            row: rows.remove(0),
            depth: 0,
        }];
        let mut edges = BTreeSet::<(usize, usize)>::new();
        let identity =
            |table: &str, row: &StaticDeleteRow| (table.to_owned(), format!("{:?}", row.key));
        let mut known = BTreeMap::from([(identity(table, &nodes[0].row), 0usize)]);
        let mut null_index = BTreeMap::<(String, String), usize>::new();
        let mut nulls = Vec::<(String, StaticDeleteRow, Vec<String>)>::new();
        let mut index = 0;
        while index < nodes.len() {
            let node_table = nodes[index].table.clone();
            let depth = nodes[index].depth + 1;
            let source_key = nodes[index].row.key.clone();
            let target = self
                .entities
                .get(node_table.as_str())
                .ok_or_else(refusal)?
                .metadata();
            for source in &physical.tables {
                for fk in source
                    .foreign_keys
                    .iter()
                    .filter(|fk| fk.target_table == node_table)
                {
                    let adapter = self
                        .entities
                        .get(source.table_name.as_str())
                        .ok_or_else(refusal)?;
                    // Every dependent the delete reads or writes needs the same
                    // request authorization as the root.
                    adapter.ensure_rls_context(ctx)?;
                    let metadata = adapter.metadata();
                    if fk.column_pairs.len() != 1
                        || metadata.primary_keys.len() != 1
                        || target.primary_keys.len() != 1
                    {
                        return Err(refusal());
                    }
                    let pair = &fk.column_pairs[0];
                    if pair.target_column != target.primary_key
                        || !metadata.relations.iter().any(|r| {
                            r.emit_foreign_key
                                && !r.is_multiple
                                && r.target_type == target.entity_name
                                && r.source_columns == [pair.source_column.as_str()]
                                && r.target_columns == [pair.target_column.as_str()]
                                && r.on_delete == fk.on_delete
                        })
                    {
                        return Err(refusal());
                    }
                    let found = adapter
                        .select(
                            &mut tx,
                            &pair.source_column,
                            source_key.clone(),
                            self.limits.max_rows() + 1,
                        )
                        .await
                        .map_err(|_| refusal())?;
                    if !found.is_empty() && depth > self.limits.max_depth() {
                        return Err(refusal());
                    }
                    for row in found {
                        match fk.on_delete {
                            DeletePolicy::Restrict => return Err(refusal()),
                            DeletePolicy::Cascade => {
                                let id = identity(&source.table_name, &row);
                                let child = if let Some(existing) = known.get(&id) {
                                    *existing
                                } else {
                                    nodes.push(Node {
                                        table: source.table_name.clone(),
                                        row,
                                        depth,
                                    });
                                    known.insert(id, nodes.len() - 1);
                                    nodes.len() - 1
                                };
                                edges.insert((child, index));
                            }
                            DeletePolicy::SetNull => {
                                let id = identity(&source.table_name, &row);
                                if let Some(existing) = null_index.get(&id) {
                                    let fields = &mut nulls[*existing].2;
                                    if !fields.contains(&pair.source_column) {
                                        fields.push(pair.source_column.clone());
                                    }
                                } else {
                                    null_index.insert(id, nulls.len());
                                    nulls.push((
                                        source.table_name.clone(),
                                        row,
                                        vec![pair.source_column.clone()],
                                    ));
                                }
                            }
                        }
                    }
                    if nodes.len() - 1 > self.limits.max_rows()
                        || nulls.len() > self.limits.max_rows()
                    {
                        return Err(refusal());
                    }
                }
            }
            index += 1;
        }
        // Topological order; every cycle (including a self-edge) refuses.
        let mut incoming = vec![0usize; nodes.len()];
        for (_, parent) in &edges {
            incoming[*parent] += 1;
        }
        let mut ready = (0..nodes.len())
            .filter(|i| incoming[*i] == 0)
            .collect::<BTreeSet<_>>();
        let mut order = Vec::with_capacity(nodes.len());
        while let Some(i) = ready.pop_first() {
            order.push(i);
            for (_, parent) in edges.range((i, 0)..=(i, usize::MAX)) {
                incoming[*parent] -= 1;
                if incoming[*parent] == 0 {
                    ready.insert(*parent);
                }
            }
        }
        if order.len() != nodes.len() {
            return Err(refusal());
        }
        nulls.retain(|(table, row, _)| !known.contains_key(&identity(table, row)));
        if nodes.len() - 1 + nulls.len() > self.limits.max_rows() {
            return Err(refusal());
        }
        let root_write = match root
            .prepare(ctx, &mut tx, nodes[0].row.clone(), Vec::new(), expected)
            .await?
        {
            StaticWriteResult::Ready(write) => write,
            StaticWriteResult::Rejected(message) => return Ok((Some(message), Vec::new())),
        };
        let mut prepared = Vec::new();
        for (table, row, fields) in nulls {
            prepared.push((
                None,
                self.entities[table.as_str()]
                    .prepare(ctx, &mut tx, row, fields, None)
                    .await
                    .map_err(|_| refusal())?
                    .dependent()?,
            ));
        }
        for i in order.into_iter().filter(|i| *i != 0) {
            let n = &nodes[i];
            prepared.push((
                Some(i),
                self.entities[n.table.as_str()]
                    .prepare(
                        ctx,
                        &mut tx,
                        n.row.clone(),
                        Vec::new(),
                        if i == 0 { expected } else { None },
                    )
                    .await
                    .map_err(|_| refusal())?
                    .dependent()?,
            ));
        }
        prepared.push((Some(0), root_write));
        // Compare the physical schema snapshot, not row data or
        // catalogue OIDs. Full backend validation is mandatory on every read:
        // the schema model alone cannot describe triggers, hidden FKs, RLS,
        // shadowing, inheritance/rules or FK comparison semantics.
        let mut notifications = Vec::new();
        for (deletion, write) in prepared {
            revalidate::<B>(&mut tx, &ownership, &physical).await?;
            // Hooks may create or re-link rows after discovery. Refuse instead
            // of letting the physical FK action perform an unplanned write.
            if let Some(i) = deletion {
                let node = &nodes[i];
                for source in &physical.tables {
                    for fk in source
                        .foreign_keys
                        .iter()
                        .filter(|fk| fk.target_table == node.table)
                    {
                        let adapter = self
                            .entities
                            .get(source.table_name.as_str())
                            .ok_or_else(refusal)?;
                        if !adapter
                            .select(
                                &mut tx,
                                &fk.column_pairs[0].source_column,
                                node.row.key.clone(),
                                1,
                            )
                            .await
                            .map_err(|_| refusal())?
                            .is_empty()
                        {
                            return Err(refusal());
                        }
                    }
                }
            }
            let outcome = if deletion == Some(0) {
                write.apply(ctx, &mut tx).await?
            } else {
                write.apply(ctx, &mut tx).await.map_err(|_| refusal())?
            };
            let notification = match outcome {
                StaticWriteResult::Ready(notification) => notification,
                StaticWriteResult::Rejected(message) if deletion == Some(0) => {
                    return Ok((Some(message), Vec::new()));
                }
                StaticWriteResult::Rejected(_) => return Err(refusal()),
            };
            if let Some(notification) = notification {
                notifications.push(notification);
            }
        }
        // The last write's After hook has no following write to guard it.
        revalidate::<B>(&mut tx, &ownership, &physical).await?;
        tx.commit_and_emit()
            .await
            .map_err(crate::graphql::errors::graphql_error_from_sqlx)?;
        Ok((None, notifications))
    }
}

// Keep the original plan: never rediscover rows or accept a changed dependency
// graph after grants and bounds have been verified. Existing transaction fences
// remain held while both full validation and snapshot comparison run.
async fn revalidate<B: StaticDeleteBackend>(
    tx: &mut MutationContext<'_, B>,
    ownership: &ManagedTableSet,
    verified: &OwnedSchemaModel,
) -> Result<()> {
    let current = B::dependencies(&mut tx.tx, ownership)
        .await
        .map_err(|_| refusal())?;
    if &current != verified {
        return Err(refusal());
    }
    Ok(())
}
