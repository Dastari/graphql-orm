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
            let unsupported: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
                    WHERE n.nspname=current_schema() AND c.relname=ANY($1)
                      AND (c.relkind<>'r' OR c.relispartition
                           OR EXISTS(SELECT 1 FROM pg_inherits i WHERE i.inhparent=c.oid OR i.inhrelid=c.oid)
                           OR EXISTS(SELECT 1 FROM pg_rewrite r WHERE r.ev_class=c.oid AND r.rulename<>'_RETURN'))
                )",
            )
            .bind(ownership.tables().collect::<Vec<_>>())
            .fetch_one(tx.as_mut())
            .await?;
            if unsupported {
                return Err(sqlx::Error::Protocol("Dependent delete refused".into()));
            }
            Ok(())
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
                let same:Option<bool>=sqlx::query_scalar("SELECT to_regclass(quote_ident($1)) = to_regclass(quote_ident(current_schema()) || '.' || quote_ident($1))").bind(table).fetch_one(tx.as_mut()).await?;
                if same != Some(true) {
                    return Err(sqlx::Error::Protocol("Dependent delete refused".into()));
                }
            }
            Self::validate_delete_relations(tx, ownership).await?;
            // Turning row_security off does not bypass RLS: PostgreSQL errors
            // rather than silently hiding dependents for a non-bypass role.
            sqlx::query("SET LOCAL row_security = off")
                .execute(tx.as_mut())
                .await?;
            super::owned_foreign_keys::validate_postgres(tx.as_mut(), ownership)
                .await
                .map_err(|_| sqlx::Error::Protocol("Dependent delete refused".into()))?;
            // Only native FK/unique/exclusion constraint triggers have modeled
            // effects. User and extension trigger bodies are opaque. The ORM's
            // own RLS search-document trigger only deletes from its search
            // table, which holds no foreign keys or entity rows; it is exempt
            // only when it fires normally, has the exact type, arguments and
            // target, and calls the unmodified ORM function.
            let owned_tables = ownership
                .tables()
                .map(|table| table.to_string())
                .collect::<Vec<_>>();
            let owned_search_tables = owned_tables
                .iter()
                .map(|table| super::search::search_table_name(table))
                .collect::<Vec<_>>();
            let triggered:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_trigger tr JOIN pg_class t ON t.oid=tr.tgrelid JOIN pg_namespace n ON n.oid=t.relnamespace JOIN pg_proc p ON p.oid=tr.tgfoid JOIN pg_namespace pn ON pn.oid=p.pronamespace LEFT JOIN pg_constraint con ON con.oid=tr.tgconstraint WHERE n.nspname=current_schema() AND t.relname=ANY($1) AND NOT COALESCE(tr.tgisinternal AND pn.nspname='pg_catalog' AND con.contype IN ('f','u','x'),false) AND NOT (NOT tr.tgisinternal AND tr.tgname=$2 AND tr.tgenabled='O' AND tr.tgtype=$4 AND tr.tgnargs=4 AND pn.nspname='graphql_orm' AND p.proname=$3 AND p.pronargs=0 AND NOT p.prosecdef AND p.prosrc=$5 AND p.proconfig=ARRAY[$6]::text[] AND EXISTS (SELECT 1 FROM unnest($1::text[], $7::text[]) AS e(base, search) WHERE e.base=t.relname AND (string_to_array(encode(tr.tgargs,'escape'),'\\000'))[1]=current_schema() AND (string_to_array(encode(tr.tgargs,'escape'),'\\000'))[2]=e.search)))").bind(&owned_tables).bind(super::rls::SEARCH_DELETE_TRIGGER).bind(super::rls::SEARCH_DELETE_FUNCTION).bind(i16::try_from(super::rls::SEARCH_DELETE_TRIGGER_TYPE).unwrap_or(0)).bind(super::rls::SEARCH_DELETE_FUNCTION_BODY).bind(super::rls::SEARCH_DELETE_FUNCTION_CONFIG).bind(&owned_search_tables).fetch_one(tx.as_mut()).await?;
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
                    SELECT 1 FROM pg_constraint con
                    JOIN pg_class t ON t.oid=con.confrelid
                    JOIN pg_namespace n ON n.oid=t.relnamespace
                    CROSS JOIN LATERAL unnest(con.conkey,con.confkey) AS pair(child_att,parent_att)
                    JOIN pg_attribute child ON child.attrelid=con.conrelid AND child.attnum=pair.child_att
                    JOIN pg_attribute parent ON parent.attrelid=con.confrelid AND parent.attnum=pair.parent_att
                    JOIN pg_type key_type ON key_type.oid=parent.atttypid
                    JOIN pg_namespace type_ns ON type_ns.oid=key_type.typnamespace
                    LEFT JOIN pg_collation cc ON cc.oid=child.attcollation
                    LEFT JOIN pg_collation pc ON pc.oid=parent.attcollation
                    WHERE con.contype='f' AND n.nspname=current_schema() AND t.relname=ANY($1)
                    AND (child.atttypid<>parent.atttypid OR child.atttypmod<>parent.atttypmod
                         OR child.attcollation<>parent.attcollation
                         OR NOT COALESCE(cc.collisdeterministic,true)
                         OR NOT COALESCE(pc.collisdeterministic,true)
                         OR type_ns.nspname<>'pg_catalog' OR key_type.typname='bpchar')
                )",
            )
            .bind(ownership.tables().collect::<Vec<_>>())
            .fetch_one(tx.as_mut())
            .await?;
            if incompatible {
                return Err(sqlx::Error::Protocol("Dependent delete refused".into()));
            }
            let physical = super::migrations::introspect_owned_postgres_connection_with_internal(
                tx.as_mut(),
                true,
            )
            .await?;
            // information_schema hides tables without privileges; pg_catalog
            // must prove that no incoming FK source disappeared from that view.
            let visible = physical
                .tables
                .iter()
                .map(|t| t.table_name.as_str())
                .collect::<Vec<_>>();
            let tables = ownership.tables().collect::<Vec<_>>();
            let hidden:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_constraint con JOIN pg_class s ON s.oid=con.conrelid JOIN pg_namespace sn ON sn.oid=s.relnamespace JOIN pg_class t ON t.oid=con.confrelid JOIN pg_namespace tn ON tn.oid=t.relnamespace WHERE con.contype='f' AND tn.nspname=current_schema() AND t.relname=ANY($1) AND (sn.nspname<>current_schema() OR NOT(s.relname=ANY($2))))").bind(tables).bind(visible).fetch_one(tx.as_mut()).await?;
            if hidden {
                return Err(sqlx::Error::Protocol("Dependent delete refused".into()));
            }
            Ok(physical)
        })
    }
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
