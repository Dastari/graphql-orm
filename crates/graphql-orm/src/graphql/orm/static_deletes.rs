//! Explicit, bounded GraphQL dependent deletes for generated static entities.
use super::*;
use crate::async_graphql::{Context, Error, Result};
use futures::future::BoxFuture;
use std::{
    any::Any,
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
    fn dependencies<'a>(
        tx: &'a mut Self::Transaction<'_>,
        ownership: &'a ManagedTableSet,
    ) -> BoxFuture<'a, crate::Result<OwnedSchemaModel>>;
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
            super::migrations::introspect_owned_sqlite_connection_with_internal(tx.as_mut(), true)
                .await
        })
    }
}
#[cfg(feature = "postgres")]
impl StaticDeleteBackend for PostgresBackend {
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
            // Turning row_security off does not bypass RLS: PostgreSQL errors
            // rather than silently hiding dependents for a non-bypass role.
            sqlx::query("SET LOCAL row_security = off")
                .execute(tx.as_mut())
                .await?;
            super::owned_foreign_keys::validate_postgres(tx.as_mut(), ownership)
                .await
                .map_err(|_| sqlx::Error::Protocol("Dependent delete refused".into()))?;
            // Only native FK/unique/exclusion constraint triggers have modeled
            // effects. User and extension trigger bodies are opaque.
            let triggered:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_trigger tr JOIN pg_class t ON t.oid=tr.tgrelid JOIN pg_namespace n ON n.oid=t.relnamespace JOIN pg_proc p ON p.oid=tr.tgfoid JOIN pg_namespace pn ON pn.oid=p.pronamespace LEFT JOIN pg_constraint con ON con.oid=tr.tgconstraint WHERE n.nspname=current_schema() AND t.relname=ANY($1) AND NOT COALESCE(tr.tgisinternal AND pn.nspname='pg_catalog' AND con.contype IN ('f','u','x'),false))").bind(ownership.tables().collect::<Vec<_>>()).fetch_one(tx.as_mut()).await?;
            if triggered {
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
/// Prepared direct GraphQL write. Authorization and before hooks have finished;
/// applying it rechecks direct GraphQL authority, performs SQL and after hooks,
/// and queues events on the same transaction.
#[doc(hidden)]
pub trait StaticPreparedWrite<B: StaticDeleteBackend>: Send {
    fn apply<'a>(
        self: Box<Self>,
        ctx: &'a Context<'_>,
        tx: &'a mut MutationContext<'_, B>,
    ) -> BoxFuture<'a, Result<StaticWriteCompletion<B>>>;
}
/// Generated capability, available only on opted-in single-key writable entities.
#[doc(hidden)]
pub trait StaticDeleteEntity<B: StaticDeleteBackend>: Entity + Send + Sync + 'static {
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
    ) -> BoxFuture<'a, Result<Box<dyn StaticPreparedWrite<B>>>>;
}
trait Adapter<B: StaticDeleteBackend>: Send + Sync {
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
    ) -> BoxFuture<'a, Result<Box<dyn StaticPreparedWrite<B>>>>;
}
struct TypedAdapter<T>(PhantomData<T>);
impl<B: StaticDeleteBackend, T: StaticDeleteEntity<B>> Adapter<B> for TypedAdapter<T> {
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
    ) -> BoxFuture<'a, Result<Box<dyn StaticPreparedWrite<B>>>> {
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
    pub async fn delete(
        &self,
        ctx: &Context<'_>,
        table: &str,
        key: SqlValue,
        expected: Option<i64>,
    ) -> Result<()> {
        let notifications = self
            .delete_inner(ctx, table, key, expected)
            .await
            .map_err(|_| refusal())?;
        let db = ctx.data::<crate::db::Database<B>>()?;
        let mut failure = None;
        for notification in notifications {
            if let Err(error) = notification.notify(ctx, db).await {
                failure.get_or_insert(error);
            }
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    async fn delete_inner(
        &self,
        ctx: &Context<'_>,
        table: &str,
        key: SqlValue,
        expected: Option<i64>,
    ) -> Result<Vec<Box<dyn StaticCommittedNotification<B>>>> {
        let db = ctx.data::<crate::db::Database<B>>()?;
        let auth = ctx.data_opt::<DbAuthContext>();
        let mut tx = MutationContext::<B>::begin(db, TransactionMode::StateMachine)
            .await
            .map_err(|_| refusal())?;
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
        let mut rows = root
            .select(&mut tx, root.metadata().primary_key, key, 2)
            .await?;
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
                        .await?;
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
        let root_write = root
            .prepare(ctx, &mut tx, nodes[0].row.clone(), Vec::new(), expected)
            .await?;
        let mut prepared = Vec::new();
        for (table, row, fields) in nulls {
            prepared.push((
                None,
                self.entities[table.as_str()]
                    .prepare(ctx, &mut tx, row, fields, None)
                    .await?,
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
                    .await?,
            ));
        }
        prepared.push((Some(0), root_write));
        let mut notifications = Vec::new();
        for (deletion, write) in prepared {
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
                            .await?
                            .is_empty()
                        {
                            return Err(refusal());
                        }
                    }
                }
            }
            if let Some(notification) = write.apply(ctx, &mut tx).await? {
                notifications.push(notification);
            }
        }
        tx.commit_and_emit().await.map_err(|_| refusal())?;
        Ok(notifications)
    }
}
