#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
use graphql_orm::graphql::orm::{MutationContext, MutationEvent, MutationHook, MutationPhase};
use graphql_orm::prelude::*;
use std::sync::{Arc, Mutex};

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    dependent_deletes,
    notify_with = "Self::enforce_resolver_assurance",
    table = "delete_parents",
    plural = "DeleteParents",
    default_sort = "id ASC"
)]
struct DeleteParent {
    #[primary_key]
    id: String,
    #[sortable]
    #[filterable(type = "string")]
    name: String,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    dependent_deletes,
    notify_with = "committed_notification",
    table = "delete_children",
    plural = "DeleteChildren",
    default_sort = "id ASC"
)]
struct DeleteChild {
    #[primary_key]
    id: String,
    #[sortable]
    #[filterable(type = "string")]
    parent_id: String,
    #[relation(
        target = "DeleteParent",
        from = "parent_id",
        to = "id",
        on_delete = "cascade"
    )]
    parent: Option<DeleteParent>,
}

impl DeleteParentMutations {
    // A user callback may share the library helper's final identifier.
    async fn enforce_resolver_assurance(
        ctx: &async_graphql::Context<'_>,
        pool: &Pool,
        entity: &DeleteParent,
        action: ChangeAction,
    ) -> async_graphql::Result<()> {
        committed_notification(ctx, pool, entity, action).await
    }
}

#[derive(Clone, Default)]
struct NotificationLog(Arc<Mutex<Vec<String>>>);
async fn committed_notification<T: Send + Sync + 'static>(
    ctx: &async_graphql::Context<'_>,
    pool: &Pool,
    _: &T,
    _: ChangeAction,
) -> async_graphql::Result<()> {
    assert!(
        ctx.data_opt::<AuthSubject>().is_some(),
        "notifications retain GraphQL context"
    );
    let parents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM delete_parents")
        .fetch_one(pool)
        .await
        .map_err(graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
    assert_eq!(
        parents, 0,
        "notify_with must run after the entire graph commits"
    );
    if let Some(log) = ctx.data_opt::<NotificationLog>() {
        log.0
            .lock()
            .unwrap()
            .push(std::any::type_name::<T>().to_owned());
    }
    Ok(())
}

#[derive(Clone, Default)]
struct Hook(
    Arc<Mutex<Vec<(&'static str, MutationPhase)>>>,
    Arc<Mutex<Option<graphql_orm::tokio::sync::broadcast::Receiver<DeleteChildChangedEvent>>>>,
);
impl MutationHook for Hook {
    fn on_mutation<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        hook_ctx: &'a mut MutationContext<'_>,
        event: &'a MutationEvent,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if let Some(rx) = self.1.lock().unwrap().as_mut() {
                assert!(rx.try_recv().is_err(), "Changed emitted before commit");
            }
            #[cfg(feature = "postgres")]
            if event.entity_name == "DeleteParent"
                && event.phase == MutationPhase::Before
                && let Some(auth) = ctx.and_then(|ctx| ctx.data_opt::<DbAuthContext>())
            {
                use graphql_orm::sqlx::Row;
                let rows = hook_ctx
                    .fetch_rows("SELECT current_setting('app.user_id', true) AS actor", &[])
                    .await
                    .map_err(graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                let actor: Option<String> = rows[0].try_get("actor").unwrap();
                assert_eq!(
                    actor, auth.user_id,
                    "native auth must be installed on the pinned transaction"
                );
            }
            if ctx.is_some_and(|ctx| ctx.data_opt::<FailAfter>().is_some())
                && event.entity_name == "DeleteParent"
                && event.phase == MutationPhase::After
            {
                return Err(async_graphql::Error::new("private hook diagnostic"));
            }
            if ctx.is_some_and(|ctx| ctx.data_opt::<LateChild>().is_some())
                && event.entity_name == "DeleteParent"
                && event.phase == MutationPhase::Before
            {
                hook_ctx
                    .execute("INSERT INTO delete_children VALUES('late','parent')", &[])
                    .await
                    .map_err(graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
            }
            if event.entity_name == "DeleteLeaf"
                && event.phase == MutationPhase::After
                && let Some(revoke) = ctx.and_then(|ctx| ctx.data_opt::<RevokeChild>())
            {
                revoke.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            if let Some(ddl) = ctx.and_then(|ctx| ctx.data_opt::<HookDdl>())
                && event.entity_name == ddl.entity
                && event.phase == ddl.phase
            {
                for sql in &ddl.statements {
                    hook_ctx
                        .execute(sql, &[])
                        .await
                        .map_err(graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                }
            }
            self.0
                .lock()
                .unwrap()
                .push((event.entity_name, event.phase.clone()));
            Ok(())
        })
    }
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn static_cascade_must_run_dependent_hooks_and_changed_events() {
    #[cfg(feature = "sqlite")]
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    #[cfg(feature = "postgres")]
    let pool = owned_postgres::OwnedPostgres::pool("dependent-delete-regression", 5)
        .await
        .unwrap();
    for statement in [
        #[cfg(feature = "sqlite")]
        "PRAGMA foreign_keys = ON",
        "CREATE TABLE delete_parents (id TEXT PRIMARY KEY, name TEXT NOT NULL)",
        "CREATE TABLE delete_children (id TEXT PRIMARY KEY, parent_id TEXT NOT NULL REFERENCES delete_parents(id) ON DELETE CASCADE)",
        "INSERT INTO delete_parents VALUES ('parent', 'parent')",
        "INSERT INTO delete_children VALUES ('child', 'parent')",
    ] {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }
    let hook = Hook::default();
    let db = graphql_orm::db::Database::with_mutation_hook(pool.clone(), hook.clone());
    let mut changes = db
        .ensure_event_sender::<DeleteChildChangedEvent>()
        .subscribe();
    let registry =
        StaticDependentDeletes::<DefaultBackend>::new(RuntimeDependentLimits::new(10, 4).unwrap())
            .register::<DeleteParent>()
            .unwrap()
            .register::<DeleteChild>()
            .unwrap();
    *hook.1.lock().unwrap() = Some(
        db.ensure_event_sender::<DeleteChildChangedEvent>()
            .subscribe(),
    );
    let notification_log = NotificationLog::default();
    let schema = schema_builder(db.clone())
        .data(notification_log.clone())
        .data(AuthSubject::new("test-principal"))
        .data(DbAuthContext {
            user_id: Some("test-principal".to_owned()),
            ..Default::default()
        })
        .data(registry)
        .finish();
    let response = schema
        .execute("mutation { deleteDeleteParent(id: \"parent\") { success } }")
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        notification_log.0.lock().unwrap().len(),
        2,
        "parent and child notifications must each run once"
    );
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM delete_children")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(remaining, 0, "physical cascade must have removed the child");
    let observed = hook.0.lock().unwrap().clone();
    let dependent_hooks = observed
        .iter()
        .filter(|(entity, _)| *entity == "DeleteChild")
        .count();
    let dependent_change = changes.try_recv();
    assert!(
        dependent_hooks == 2 && dependent_change.is_ok(),
        "dependent hooks: {dependent_hooks}, dependent Changed: {dependent_change:?}, all hooks: {observed:?}"
    );
}

schema_roots! { query_custom_ops: [], entities: [DeleteParent,DeleteChild,DeleteLeaf,DeleteNull,DeleteCycle], }

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    dependent_deletes,
    notify_with = "committed_notification",
    table = "delete_nulls",
    plural = "DeleteNulls",
    default_sort = "id ASC"
)]
struct DeleteNull {
    #[primary_key]
    id: String,
    #[sortable]
    #[filterable(type = "string")]
    #[graphql_orm(write_policy = "null_guard")]
    parent_id: Option<String>,
    #[relation(
        target = "DeleteParent",
        from = "parent_id",
        to = "id",
        on_delete = "set_null"
    )]
    parent: Option<DeleteParent>,
}
#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    dependent_deletes,
    notify_with = "committed_notification",
    table = "delete_leaves",
    plural = "DeleteLeaves",
    default_sort = "id ASC"
)]
struct DeleteLeaf {
    #[primary_key]
    id: String,
    #[sortable]
    #[filterable(type = "string")]
    child_id: String,
    #[relation(
        target = "DeleteChild",
        from = "child_id",
        to = "id",
        on_delete = "cascade"
    )]
    child: Option<DeleteChild>,
}
#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    dependent_deletes,
    table = "delete_cycles",
    plural = "DeleteCycles",
    default_sort = "id ASC"
)]
struct DeleteCycle {
    #[primary_key]
    id: String,
    #[sortable]
    #[filterable(type = "string")]
    parent_id: Option<String>,
    #[relation(
        target = "DeleteCycle",
        from = "parent_id",
        to = "id",
        on_delete = "cascade"
    )]
    parent: Option<Box<DeleteCycle>>,
}
struct Policy {
    deny_entity: bool,
    deny_field: bool,
}
impl EntityPolicy for Policy {
    fn can_access_entity<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        kind: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            assert!(ctx.is_some());
            assert_eq!(surface, EntityAccessSurface::GraphqlMutation);
            let revoked = ctx
                .and_then(|ctx| ctx.data_opt::<RevokeChild>())
                .is_some_and(|revoke| revoke.0.load(std::sync::atomic::Ordering::SeqCst));
            Ok(!((self.deny_entity || revoked)
                && entity == "DeleteChild"
                && kind == EntityAccessKind::Write))
        })
    }
}
impl FieldPolicy for Policy {
    fn ignore_denied_field_write(&self, _: &'static str, _: &'static str) -> bool {
        true
    }
    fn can_read_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn can_write_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(!self.deny_field) })
    }
}
#[cfg(feature = "sqlite")]
type Pool = sqlx::SqlitePool;
#[cfg(feature = "postgres")]
type Pool = sqlx::PgPool;
async fn fixture(label: &str) -> Pool {
    #[cfg(feature = "sqlite")]
    let pool = {
        let _ = label;
        sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    };
    #[cfg(feature = "postgres")]
    let pool = owned_postgres::OwnedPostgres::pool(label, 5).await.unwrap();
    for sql in [
        "CREATE TABLE delete_parents(id TEXT PRIMARY KEY,name TEXT NOT NULL)",
        "CREATE TABLE delete_children(id TEXT PRIMARY KEY,parent_id TEXT NOT NULL REFERENCES delete_parents(id) ON DELETE CASCADE)",
        "CREATE TABLE delete_leaves(id TEXT PRIMARY KEY,child_id TEXT NOT NULL REFERENCES delete_children(id) ON DELETE CASCADE)",
        "CREATE TABLE delete_nulls(id TEXT PRIMARY KEY,parent_id TEXT REFERENCES delete_parents(id) ON DELETE SET NULL)",
        "CREATE TABLE delete_cycles(id TEXT PRIMARY KEY,parent_id TEXT REFERENCES delete_cycles(id) ON DELETE CASCADE)",
        "INSERT INTO delete_parents VALUES('parent','parent')",
        "INSERT INTO delete_children VALUES('child','parent')",
        "INSERT INTO delete_leaves VALUES('leaf','child')",
        "INSERT INTO delete_nulls VALUES('null','parent')",
    ] {
        sqlx::query(sql).execute(&pool).await.unwrap();
    }
    pool
}
async fn run_case(
    label: &str,
    rows: usize,
    depth: usize,
    deny_entity: bool,
    deny_field: bool,
) -> (Pool, async_graphql::Response, Hook) {
    let pool = fixture(label).await;
    let hook = Hook::default();
    let mut db = Database::with_mutation_hook(pool.clone(), hook.clone());
    db.set_entity_policy(Policy {
        deny_entity,
        deny_field,
    });
    db.set_field_policy(Policy {
        deny_entity,
        deny_field,
    });
    let registry = StaticDependentDeletes::<DefaultBackend>::new(
        RuntimeDependentLimits::new(rows, depth).unwrap(),
    )
    .register::<DeleteParent>()
    .unwrap()
    .register::<DeleteChild>()
    .unwrap()
    .register::<DeleteLeaf>()
    .unwrap()
    .register::<DeleteNull>()
    .unwrap();
    let mut child_changes = db
        .ensure_event_sender::<DeleteChildChangedEvent>()
        .subscribe();
    *hook.1.lock().unwrap() = Some(
        db.ensure_event_sender::<DeleteChildChangedEvent>()
            .subscribe(),
    );
    let schema = schema_builder(db)
        .data(AuthSubject::new("test-principal"))
        .data(registry)
        .finish();
    let response = schema
        .execute("mutation { deleteDeleteParent(id: \"parent\") { success } }")
        .await;
    if !response.errors.is_empty() {
        assert!(child_changes.try_recv().is_err());
    }
    (pool, response, hook)
}
async fn assert_intact(pool: &Pool) {
    for table in [
        "delete_parents",
        "delete_children",
        "delete_leaves",
        "delete_nulls",
    ] {
        let n: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(n, 1, "{table}");
    }
    let parent: Option<String> = sqlx::query_scalar("SELECT parent_id FROM delete_nulls")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(parent.as_deref(), Some("parent"));
}
#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn multilevel_and_mixed_dependents() {
    let (pool, response, hook) = run_case("dependent-mixed", 10, 4, false, false).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let events = hook.0.lock().unwrap().clone();
    for entity in ["DeleteParent", "DeleteChild", "DeleteLeaf", "DeleteNull"] {
        assert_eq!(
            events.iter().filter(|(n, _)| *n == entity).count(),
            2,
            "{events:?}"
        );
    }
    let parent: Option<String> = sqlx::query_scalar("SELECT parent_id FROM delete_nulls")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(parent, None);
}
#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn row_and_depth_limits_refuse_without_writes() {
    for (label, rows, depth) in [
        ("dependent-row-limit", 1, 4),
        ("dependent-depth-limit", 10, 1),
    ] {
        let (pool, response, hook) = run_case(label, rows, depth, false, false).await;
        assert_eq!(response.errors[0].message, "Dependent delete refused");
        assert_intact(&pool).await;
        assert!(hook.0.lock().unwrap().is_empty());
    }
}
#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn unauthorized_dependent_refuses_without_writes() {
    let (pool, response, _) = run_case("dependent-entity-denied", 10, 4, true, false).await;
    assert_eq!(response.errors[0].message, "Dependent delete refused");
    assert_intact(&pool).await;
}
#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn denied_set_null_refuses_even_with_legacy_field_omission() {
    let (pool, response, _) = run_case("dependent-field-denied", 10, 4, false, true).await;
    assert_eq!(response.errors[0].message, "Dependent delete refused");
    assert_intact(&pool).await;
}
#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn cycles_refuse() {
    let pool = fixture("dependent-cycle").await;
    sqlx::query("INSERT INTO delete_cycles VALUES('cycle','cycle')")
        .execute(&pool)
        .await
        .unwrap();
    let db = Database::new(pool.clone());
    let registry =
        StaticDependentDeletes::<DefaultBackend>::new(RuntimeDependentLimits::new(10, 4).unwrap())
            .register::<DeleteCycle>()
            .unwrap();
    let schema = schema_builder(db)
        .data(AuthSubject::new("test-principal"))
        .data(registry)
        .finish();
    let response = schema
        .execute("mutation { deleteDeleteCycle(id: \"cycle\") { success } }")
        .await;
    assert_eq!(response.errors[0].message, "Dependent delete refused");
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM delete_cycles")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

struct FailAfter;
#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn rollback_discards_dependent_events() {
    let pool = fixture("dependent-after-hook-refusal").await;
    let hook = Hook::default();
    let mut db = Database::with_mutation_hook(pool.clone(), hook);
    db.set_field_policy(Policy {
        deny_entity: false,
        deny_field: false,
    });
    let mut changes = db
        .ensure_event_sender::<DeleteChildChangedEvent>()
        .subscribe();
    let registry =
        StaticDependentDeletes::<DefaultBackend>::new(RuntimeDependentLimits::new(10, 4).unwrap())
            .register::<DeleteParent>()
            .unwrap()
            .register::<DeleteChild>()
            .unwrap()
            .register::<DeleteLeaf>()
            .unwrap()
            .register::<DeleteNull>()
            .unwrap();
    let schema = schema_builder(db)
        .data(AuthSubject::new("test-principal"))
        .data(registry)
        .finish();
    let response = schema
        .execute(
            async_graphql::Request::new(
                "mutation { deleteDeleteParent(id: \"parent\") { success } }",
            )
            .data(FailAfter),
        )
        .await;
    assert_eq!(response.errors[0].message, "private hook diagnostic");
    assert_intact(&pool).await;
    assert!(changes.try_recv().is_err());
}
#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn unknown_dependent_refuses() {
    let pool = fixture("dependent-unknown").await;
    sqlx::query("CREATE TABLE unknown_children(id TEXT PRIMARY KEY,parent_id TEXT REFERENCES delete_parents(id) ON DELETE CASCADE)").execute(&pool).await.unwrap();
    let db = Database::new(pool.clone());
    let registry =
        StaticDependentDeletes::<DefaultBackend>::new(RuntimeDependentLimits::new(10, 4).unwrap())
            .register::<DeleteParent>()
            .unwrap()
            .register::<DeleteChild>()
            .unwrap()
            .register::<DeleteLeaf>()
            .unwrap()
            .register::<DeleteNull>()
            .unwrap();
    let response = schema_builder(db)
        .data(AuthSubject::new("test-principal"))
        .data(registry)
        .finish()
        .execute("mutation { deleteDeleteParent(id: \"parent\") { success } }")
        .await;
    assert_eq!(response.errors[0].message, "Dependent delete refused");
    assert_intact(&pool).await;
}

struct LateChild;
#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn hook_inserted_dependents_refuse_instead_of_using_physical_cascade() {
    let pool = fixture("dependent-hook-insertion").await;
    let mut db = Database::with_mutation_hook(pool.clone(), Hook::default());
    db.set_field_policy(Policy {
        deny_entity: false,
        deny_field: false,
    });
    let mut changes = db
        .ensure_event_sender::<DeleteChildChangedEvent>()
        .subscribe();
    let registry =
        StaticDependentDeletes::<DefaultBackend>::new(RuntimeDependentLimits::new(10, 4).unwrap())
            .register::<DeleteParent>()
            .unwrap()
            .register::<DeleteChild>()
            .unwrap()
            .register::<DeleteLeaf>()
            .unwrap()
            .register::<DeleteNull>()
            .unwrap();
    let schema = schema_builder(db)
        .data(AuthSubject::new("test-principal"))
        .data(registry)
        .finish();
    let response = schema
        .execute(
            async_graphql::Request::new(
                "mutation { deleteDeleteParent(id: \"parent\") { success } }",
            )
            .data(LateChild),
        )
        .await;
    assert_eq!(response.errors[0].message, "Dependent delete refused");
    assert_intact(&pool).await;
    assert!(changes.try_recv().is_err());
}

struct RevokeChild(std::sync::atomic::AtomicBool);
#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn dependent_authority_is_rechecked_after_earlier_hooks() {
    let pool = fixture("dependent-grant-revocation").await;
    let mut db = Database::with_mutation_hook(pool.clone(), Hook::default());
    db.set_entity_policy(Policy {
        deny_entity: false,
        deny_field: false,
    });
    db.set_field_policy(Policy {
        deny_entity: false,
        deny_field: false,
    });
    let mut changes = db
        .ensure_event_sender::<DeleteChildChangedEvent>()
        .subscribe();
    let registry =
        StaticDependentDeletes::<DefaultBackend>::new(RuntimeDependentLimits::new(10, 4).unwrap())
            .register::<DeleteParent>()
            .unwrap()
            .register::<DeleteChild>()
            .unwrap()
            .register::<DeleteLeaf>()
            .unwrap()
            .register::<DeleteNull>()
            .unwrap();
    let schema = schema_builder(db)
        .data(AuthSubject::new("test-principal"))
        .data(registry)
        .finish();
    let response = schema
        .execute(
            async_graphql::Request::new(
                "mutation { deleteDeleteParent(id: \"parent\") { success } }",
            )
            .data(RevokeChild(std::sync::atomic::AtomicBool::new(false))),
        )
        .await;
    assert_eq!(response.errors[0].message, "Dependent delete refused");
    assert_intact(&pool).await;
    assert!(changes.try_recv().is_err());
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn temporary_table_shadowing_refuses_case_insensitively() {
    let pool = fixture("dependent-temporary-shadow").await;
    sqlx::query("CREATE TEMP TABLE DELETE_PARENTS(id TEXT PRIMARY KEY,name TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO temp.DELETE_PARENTS VALUES('parent','shadow')")
        .execute(&pool)
        .await
        .unwrap();
    let db = Database::new(pool.clone());
    let registry =
        StaticDependentDeletes::<DefaultBackend>::new(RuntimeDependentLimits::new(10, 4).unwrap())
            .register::<DeleteParent>()
            .unwrap()
            .register::<DeleteChild>()
            .unwrap()
            .register::<DeleteLeaf>()
            .unwrap()
            .register::<DeleteNull>()
            .unwrap();
    let response = schema_builder(db)
        .data(AuthSubject::new("test-principal"))
        .data(registry)
        .finish()
        .execute("mutation { deleteDeleteParent(id: \"parent\") { success } }")
        .await;
    assert_eq!(response.errors[0].message, "Dependent delete refused");
    assert_intact(&pool).await;
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM main.delete_parents")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn database_triggers_cannot_create_unplanned_physical_cascades() {
    let pool = fixture("dependent-database-trigger").await;
    sqlx::query("DELETE FROM delete_nulls")
        .execute(&pool)
        .await
        .unwrap();
    #[cfg(feature="sqlite")]
    sqlx::query("CREATE TRIGGER spawn_dependent BEFORE DELETE ON delete_children BEGIN INSERT INTO delete_leaves VALUES('late-trigger',OLD.id); END").execute(&pool).await.unwrap();
    #[cfg(feature = "postgres")]
    {
        sqlx::query("CREATE FUNCTION spawn_dependent() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO delete_leaves VALUES('late-trigger',OLD.id); RETURN OLD; END $$").execute(&pool).await.unwrap();
        sqlx::query("CREATE TRIGGER spawn_dependent BEFORE DELETE ON delete_children FOR EACH ROW EXECUTE FUNCTION spawn_dependent()").execute(&pool).await.unwrap();
    }
    let db = Database::new(pool.clone());
    let mut changes = db
        .ensure_event_sender::<DeleteChildChangedEvent>()
        .subscribe();
    let registry =
        StaticDependentDeletes::<DefaultBackend>::new(RuntimeDependentLimits::new(2, 4).unwrap())
            .register::<DeleteParent>()
            .unwrap()
            .register::<DeleteChild>()
            .unwrap()
            .register::<DeleteLeaf>()
            .unwrap()
            .register::<DeleteNull>()
            .unwrap();
    let response = schema_builder(db)
        .data(AuthSubject::new("test-principal"))
        .data(registry)
        .finish()
        .execute("mutation { deleteDeleteParent(id: \"parent\") { success } }")
        .await;
    assert!(
        !response.errors.is_empty(),
        "unmodeled trigger can create a third dependent beyond the two-row limit"
    );
    assert_eq!(response.errors[0].message, "Dependent delete refused");
    for table in ["delete_parents", "delete_children", "delete_leaves"] {
        let n: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1);
    }
    assert!(changes.try_recv().is_err());
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn bulk_delete_cannot_bypass_opted_in_dependent_authorization() {
    let pool = fixture("dependent-bulk-bypass").await;
    let db = Database::with_entity_policy(
        pool.clone(),
        Policy {
            deny_entity: true,
            deny_field: false,
        },
    );
    let registry =
        StaticDependentDeletes::<DefaultBackend>::new(RuntimeDependentLimits::new(10, 4).unwrap())
            .register::<DeleteParent>()
            .unwrap()
            .register::<DeleteChild>()
            .unwrap()
            .register::<DeleteLeaf>()
            .unwrap()
            .register::<DeleteNull>()
            .unwrap();
    let schema = schema_builder(db)
        .data(AuthSubject::new("test-principal"))
        .data(registry)
        .finish();
    let response = schema
        .execute("mutation { deleteDeleteParents(where: {name: {eq: \"parent\"}}) { __typename } }")
        .await;
    assert!(
        !response.errors.is_empty(),
        "bulk delete must not bypass denied dependent writes: {}",
        response.data
    );
    assert!(!schema.sdl().contains("deleteDeleteParents("));
    assert!(
        graphql_orm_operation_catalog()
            .operations()
            .iter()
            .all(|op| op.field_name() != "deleteDeleteParents")
    );
    assert_intact(&pool).await;
}

mod opt_out {
    use graphql_orm::graphql::orm::{MutationContext, MutationEvent, MutationHook, MutationPhase};
    use graphql_orm::prelude::*;
    use std::sync::{Arc, Mutex};

    #[derive(
        GraphQLEntity,
        GraphQLOperations,
        serde::Serialize,
        serde::Deserialize,
        Clone,
        Debug,
        PartialEq,
    )]
    #[graphql_entity(
        table = "delete_parents",
        plural = "DeleteParents",
        default_sort = "id ASC"
    )]
    struct DeleteParent {
        #[primary_key]
        id: String,
        #[sortable]
        #[filterable(type = "string")]
        name: String,
    }

    #[derive(
        GraphQLEntity,
        GraphQLOperations,
        serde::Serialize,
        serde::Deserialize,
        Clone,
        Debug,
        PartialEq,
    )]
    #[graphql_entity(
        table = "delete_children",
        plural = "DeleteChildren",
        default_sort = "id ASC"
    )]
    struct DeleteChild {
        #[primary_key]
        id: String,
        #[sortable]
        #[filterable(type = "string")]
        parent_id: String,
        #[relation(
            target = "DeleteParent",
            from = "parent_id",
            to = "id",
            on_delete = "cascade"
        )]
        parent: Option<DeleteParent>,
    }

    #[derive(Clone, Default)]
    struct Hook(Arc<Mutex<Vec<(&'static str, MutationPhase)>>>);
    impl MutationHook for Hook {
        fn on_mutation<'a>(
            &'a self,
            _: Option<&'a async_graphql::Context<'_>>,
            _: &'a mut MutationContext<'_>,
            event: &'a MutationEvent,
        ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
            Box::pin(async move {
                self.0
                    .lock()
                    .unwrap()
                    .push((event.entity_name, event.phase.clone()));
                Ok(())
            })
        }
    }

    #[tokio::test]
    #[cfg_attr(
        feature = "postgres",
        ignore = "requires test-owned PostgreSQL container"
    )]
    async fn opt_out_still_uses_physical_cascade() {
        #[cfg(feature = "sqlite")]
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        #[cfg(feature = "postgres")]
        let pool = super::owned_postgres::OwnedPostgres::pool("dependent-opt-out", 5)
            .await
            .unwrap();
        for statement in [
            #[cfg(feature = "sqlite")]
            "PRAGMA foreign_keys = ON",
            "CREATE TABLE delete_parents (id TEXT PRIMARY KEY, name TEXT NOT NULL)",
            "CREATE TABLE delete_children (id TEXT PRIMARY KEY, parent_id TEXT NOT NULL REFERENCES delete_parents(id) ON DELETE CASCADE)",
            "INSERT INTO delete_parents VALUES ('parent', 'parent')",
            "INSERT INTO delete_children VALUES ('child', 'parent')",
        ] {
            sqlx::query(statement).execute(&pool).await.unwrap();
        }
        let hook = Hook::default();
        let db = graphql_orm::db::Database::with_mutation_hook(pool.clone(), hook.clone());
        let mut changes = db
            .ensure_event_sender::<DeleteChildChangedEvent>()
            .subscribe();
        assert!(
            DeleteParent::delete_by_id(&db, &"parent".to_owned())
                .await
                .unwrap()
        );
        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM delete_children")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, 0, "physical cascade must have removed the child");
        let observed = hook.0.lock().unwrap().clone();
        let dependent_hooks = observed
            .iter()
            .filter(|(entity, _)| *entity == "DeleteChild")
            .count();
        let dependent_change = changes.try_recv();
        assert!(
            dependent_hooks == 0 && dependent_change.is_err(),
            "dependent hooks: {dependent_hooks}, dependent Changed: {dependent_change:?}, all hooks: {observed:?}"
        );
    }
}

// Regressions derived from the independent PR #388 assurance/collation probes.
#[derive(Debug)]
struct ReviewAssurance(Arc<Mutex<Vec<String>>>);
impl graphql_orm::graphql::assurance::AssuranceRequirementEvaluator for ReviewAssurance {
    fn enforce(
        &self,
        _: &graphql_orm::graphql::assurance::AssuranceCheck<'_>,
        _: graphql_orm::graphql::assurance::AssuranceActorClass,
        policy: &str,
    ) -> async_graphql::Result<()> {
        let mut checks = self.0.lock().unwrap();
        checks.push(policy.to_owned());
        let count = checks.iter().filter(|p| p.as_str() == policy).count();
        if policy == "step-up" || (policy == "recheck" && count > 1) {
            Err(async_graphql::Error::new("step-up required"))
        } else {
            Ok(())
        }
    }
}
async fn review_assurance_case(
    root: Option<&str>,
    child: Option<&str>,
    null: Option<&str>,
    strict: bool,
    succeeds: bool,
) {
    use graphql_orm::graphql::assurance::*;
    let pool = fixture("dependent-assurance-review").await;
    let checks = Arc::new(Mutex::new(Vec::new()));
    // A strict registry may classify the root but omit dependent operations
    // (for example, a host composing its own partial operation catalogue).
    let catalog = GraphqlOperationCatalog::compose(std::iter::empty());
    let mut builder = OperationAssuranceRegistry::builder(
        &catalog,
        AssuranceSchemaConfig::legacy().with_strict_mutation_classification(strict),
    );
    for (field, class) in [
        ("deleteDeleteParent", root),
        ("deleteDeleteChild", child),
        ("updateDeleteNull", null),
        ("deleteDeleteLeaf", Some("exempt")),
    ] {
        if let Some(class) = class {
            builder
                .register_custom(
                    field,
                    GraphqlOperationKind::Mutation,
                    field,
                    AssuranceActorClass::Interactive,
                )
                .unwrap();
            if class == "exempt" {
                builder
                    .exempt(GraphqlOperationKind::Mutation, field, "test exemption")
                    .unwrap();
            } else {
                builder
                    .require(GraphqlOperationKind::Mutation, field, class)
                    .unwrap();
            }
        }
    }
    let enforcement = AssuranceEnforcement::new(
        Arc::new(builder.build().unwrap()),
        Arc::new(ReviewAssurance(checks.clone())),
    );
    let mut db = Database::new(pool.clone());
    db.set_field_policy(Policy {
        deny_entity: false,
        deny_field: false,
    });
    let mut child_events = db
        .ensure_event_sender::<DeleteChildChangedEvent>()
        .subscribe();
    let mut null_events = db
        .ensure_event_sender::<DeleteNullChangedEvent>()
        .subscribe();
    let registry =
        StaticDependentDeletes::<DefaultBackend>::new(RuntimeDependentLimits::new(10, 4).unwrap())
            .register::<DeleteParent>()
            .unwrap()
            .register::<DeleteChild>()
            .unwrap()
            .register::<DeleteNull>()
            .unwrap()
            .register::<DeleteLeaf>()
            .unwrap();
    let schema = schema_builder(db)
        .data(AuthSubject::new("test-principal"))
        .data(enforcement)
        .data(registry)
        .finish();
    // Direct controls: the same dependent coordinate must be refused directly.
    for (class, document) in [
        (
            child,
            "mutation { deleteDeleteChild(id:\"child\") { success } }",
        ),
        (
            null,
            "mutation { updateDeleteNull(id:\"null\", input:{parentId:null}) { success } }",
        ),
    ] {
        if class == Some("step-up") || (strict && class.is_none()) {
            assert!(!schema.execute(document).await.errors.is_empty());
        }
    }
    checks.lock().unwrap().clear();
    let response = schema
        .execute("mutation { deleteDeleteParent(id:\"parent\") { success } }")
        .await;
    if succeeds {
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        // The required root is checked at admission, pinned preparation and
        // immediately before SQL; exempt dependents must not borrow it.
        let observed = checks.lock().unwrap();
        for (class, count) in [(root, 3), (child, 2), (null, 2)] {
            if let Some(class) = class.filter(|c| *c != "exempt") {
                assert_eq!(
                    observed.iter().filter(|p| p.as_str() == class).count(),
                    count,
                    "{observed:?}"
                );
            }
        }
        assert!(child_events.try_recv().is_ok());
        assert!(null_events.try_recv().is_ok());
    } else {
        assert!(
            !response.errors.is_empty(),
            "dependent assurance was bypassed: {:?}",
            response.data
        );
        if root.is_some() && root != Some("step-up") {
            assert_eq!(response.errors[0].message, "Dependent delete refused");
            assert!(response.errors[0].extensions.is_none());
        }
        assert_intact(&pool).await;
        assert!(child_events.try_recv().is_err());
        assert!(null_events.try_recv().is_err());
    }
}
#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn review_dependent_assurance_uses_its_own_coordinate() {
    review_assurance_case(
        Some("exempt"),
        Some("step-up"),
        Some("exempt"),
        false,
        false,
    )
    .await;
    review_assurance_case(
        Some("exempt"),
        Some("exempt"),
        Some("step-up"),
        false,
        false,
    )
    .await;
    review_assurance_case(
        Some("exempt"),
        Some("child-approved"),
        Some("null-approved"),
        true,
        true,
    )
    .await;
    review_assurance_case(
        Some("exempt"),
        Some("recheck"),
        Some("exempt"),
        false,
        false,
    )
    .await;
    review_assurance_case(
        Some("exempt"),
        Some("exempt"),
        Some("recheck"),
        false,
        false,
    )
    .await;
}
#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn review_required_root_does_not_override_exempt_dependents() {
    review_assurance_case(
        Some("root-approved"),
        Some("exempt"),
        Some("exempt"),
        false,
        true,
    )
    .await;
    review_assurance_case(
        Some("root-approved"),
        Some("exempt"),
        Some("exempt"),
        true,
        true,
    )
    .await;
    review_assurance_case(
        Some("step-up"),
        Some("exempt"),
        Some("exempt"),
        false,
        false,
    )
    .await;
}
#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn review_strict_assurance_refuses_unregistered_dependent_and_root() {
    review_assurance_case(Some("exempt"), None, Some("exempt"), true, false).await;
    review_assurance_case(Some("exempt"), Some("exempt"), None, true, false).await;
    review_assurance_case(None, Some("exempt"), Some("exempt"), true, false).await;
}

async fn review_comparison_case(parent_type: &str, child_type: &str, parent: &str, child: &str) {
    #[cfg(feature = "sqlite")]
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    #[cfg(feature = "postgres")]
    let pool = owned_postgres::OwnedPostgres::pool("dependent-collation-review", 5)
        .await
        .unwrap();
    #[cfg(feature = "postgres")]
    sqlx::query(
        "CREATE COLLATION review_ci (provider=icu, locale='und-u-ks-level2', deterministic=false)",
    )
    .execute(&pool)
    .await
    .unwrap();
    #[cfg(feature = "postgres")]
    if parent_type == "citext" {
        sqlx::query("CREATE EXTENSION citext")
            .execute(&pool)
            .await
            .unwrap();
    }
    let (parent_type, table_option) = parent_type
        .strip_suffix(" STRICT")
        .map(|ty| (ty, " STRICT"))
        .unwrap_or((parent_type, ""));
    for sql in [
        format!(
            "CREATE TABLE delete_parents(id {parent_type} PRIMARY KEY,name TEXT NOT NULL){table_option}"
        ),
        format!(
            "CREATE TABLE delete_children(id TEXT PRIMARY KEY,parent_id {child_type} NOT NULL REFERENCES delete_parents(id) ON DELETE CASCADE)"
        ),
        format!(
            "CREATE TABLE delete_nulls(id TEXT PRIMARY KEY,parent_id {child_type} REFERENCES delete_parents(id) ON DELETE SET NULL)"
        ),
        format!("INSERT INTO delete_parents VALUES('{parent}','parent')"),
        format!("INSERT INTO delete_children VALUES('child','{child}')"),
        format!("INSERT INTO delete_nulls VALUES('null','{child}')"),
    ] {
        sqlx::query(&sql).execute(&pool).await.unwrap();
    }
    let hooks = Hook::default();
    let mut db = Database::with_mutation_hook(pool.clone(), hooks.clone());
    db.set_entity_policy(Policy {
        deny_entity: true,
        deny_field: false,
    });
    let mut events = db
        .ensure_event_sender::<DeleteChildChangedEvent>()
        .subscribe();
    let registry =
        StaticDependentDeletes::<DefaultBackend>::new(RuntimeDependentLimits::new(10, 4).unwrap())
            .register::<DeleteParent>()
            .unwrap()
            .register::<DeleteChild>()
            .unwrap()
            .register::<DeleteNull>()
            .unwrap();
    let schema = schema_builder(db)
        .data(AuthSubject::new("test-principal"))
        .data(registry)
        .finish();
    assert!(
        !schema
            .execute("mutation { deleteDeleteChild(id:\"child\") { success } }")
            .await
            .errors
            .is_empty()
    );
    let response = schema
        .execute(format!(
            "mutation {{ deleteDeleteParent(id:\"{parent}\") {{ success }} }}"
        ))
        .await;
    assert!(
        !response.errors.is_empty(),
        "unsupported FK comparison was not refused before writes: {:?}",
        response.data
    );
    for table in ["delete_parents", "delete_children", "delete_nulls"] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "{table}");
    }
    let linked: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM delete_nulls WHERE parent_id IS NOT NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(linked, 1);
    assert!(hooks.0.lock().unwrap().is_empty());
    assert!(events.try_recv().is_err());
}
#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn review_unsafe_fk_collations_refuse_before_planning_or_recheck() {
    #[cfg(feature = "sqlite")]
    {
        review_comparison_case("TEXT COLLATE NOCASE", "TEXT", "P", "p").await;
        review_comparison_case("TEXT", "TEXT COLLATE NOCASE", "P", "P").await;
        review_comparison_case("TEXT COLLATE BINARY", "TEXT", "P", "P").await;
    }
    #[cfg(feature = "postgres")]
    {
        review_comparison_case("TEXT COLLATE review_ci", "TEXT", "P", "p").await;
        review_comparison_case("TEXT", "TEXT COLLATE review_ci", "P", "P").await;
        review_comparison_case("TEXT COLLATE review_ci", "TEXT COLLATE review_ci", "P", "p").await;
    }
}
#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn review_unsafe_fk_type_or_affinity_refuses() {
    #[cfg(feature = "sqlite")]
    {
        review_comparison_case("INTEGER", "TEXT", "1", "01").await;
        review_comparison_case("TEXT", "NUMERIC", "P", "P").await;
        review_comparison_case("ANY STRICT", "NUMERIC", "P", "P").await;
    }
    #[cfg(feature = "postgres")]
    review_comparison_case("TEXT", "VARCHAR(64)", "P", "P").await;
}

#[derive(Default)]
struct ReviewDenyRootRow;
impl RowPolicy for ReviewDenyRootRow {
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            assert_eq!(
                entity, "DeleteParent",
                "denied root must precede dependent authorization"
            );
            Ok(false)
        })
    }
}
#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn review_root_row_denial_precedes_graph_locks_and_discovery() {
    let pool = fixture("root-preflight-review").await;
    #[cfg(feature = "postgres")]
    let mut blocker = pool.begin().await.unwrap();
    #[cfg(feature = "postgres")]
    sqlx::query("LOCK TABLE delete_children IN ROW EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    #[cfg(feature = "sqlite")]
    sqlx::query("DROP TABLE delete_leaves")
        .execute(&pool)
        .await
        .unwrap();
    let mut db = Database::new(pool.clone());
    db.set_row_policy(ReviewDenyRootRow);
    let registry =
        StaticDependentDeletes::<DefaultBackend>::new(RuntimeDependentLimits::new(10, 4).unwrap())
            .register::<DeleteParent>()
            .unwrap()
            .register::<DeleteChild>()
            .unwrap()
            .register::<DeleteLeaf>()
            .unwrap();
    let schema = schema_builder(db)
        .data(AuthSubject::new("test-principal"))
        .data(registry)
        .finish();
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        schema.execute("mutation { deleteDeleteParent(id:\"parent\") { success } }"),
    )
    .await
    .expect("denied root must not wait for graph locks");
    assert_eq!(response.errors.len(), 1);
    assert_eq!(response.errors[0].message, "forbidden");
    #[cfg(feature = "postgres")]
    blocker.rollback().await.unwrap();
}

mod review_root_errors {
    use super::*;
    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        dependent_deletes,
        table = "review_root_errors",
        plural = "ReviewRoots",
        default_sort = "id ASC"
    )]
    struct ReviewRoot {
        #[primary_key]
        id: String,
        #[sortable]
        #[filterable(type = "string")]
        name: String,
        #[graphql_orm(version = "managed", default = "0")]
        version: i64,
    }
    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "review_root_errors",
        plural = "DirectRoots",
        default_sort = "id ASC"
    )]
    struct DirectRoot {
        #[primary_key]
        id: String,
        #[sortable]
        #[filterable(type = "string")]
        name: String,
        #[graphql_orm(version = "managed", default = "0")]
        version: i64,
    }
    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        dependent_deletes,
        table = "review_root_errors",
        plural = "UnregisteredRoots",
        default_sort = "id ASC"
    )]
    struct UnregisteredRoot {
        #[primary_key]
        id: String,
        #[sortable]
        #[filterable(type = "string")]
        name: String,
    }
    schema_roots! { query_custom_ops: [], entities: [ReviewRoot,DirectRoot,UnregisteredRoot], }
    async fn root_schema() -> (
        Pool,
        async_graphql::Schema<QueryRoot, MutationRoot, SubscriptionRoot>,
    ) {
        #[cfg(feature = "sqlite")]
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        #[cfg(feature = "postgres")]
        let pool = owned_postgres::OwnedPostgres::pool("root-errors-review", 5)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE review_root_errors(id TEXT PRIMARY KEY,name TEXT NOT NULL,version BIGINT NOT NULL DEFAULT 0)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO review_root_errors VALUES('root','root',2)")
            .execute(&pool)
            .await
            .unwrap();
        let registry = StaticDependentDeletes::<DefaultBackend>::new(
            RuntimeDependentLimits::new(10, 4).unwrap(),
        )
        .register::<ReviewRoot>()
        .unwrap();
        let schema = schema_builder(Database::new(pool.clone()))
            .data(AuthSubject::new("test-principal"))
            .data(registry)
            .finish();
        (pool, schema)
    }
    #[tokio::test]
    #[cfg_attr(
        feature = "postgres",
        ignore = "requires test-owned PostgreSQL container"
    )]
    async fn review_root_registry_cannot_substitute_a_different_entity_on_the_same_table() {
        let (pool, schema) = root_schema().await;
        let response = schema
            .execute("mutation { deleteUnregisteredRoot(id: \"root\") { success } }")
            .await;
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM review_root_errors")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(
            !response.errors.is_empty(),
            "unregistered root reused a different writer: {:?}; rows left={count}",
            response.data
        );
        assert_eq!(response.errors[0].message, "Dependent delete refused");
        assert_eq!(count, 1);
    }
    #[tokio::test]
    #[cfg_attr(
        feature = "postgres",
        ignore = "requires test-owned PostgreSQL container"
    )]
    async fn review_root_errors_match_direct_not_found_and_conflict() {
        let (pool, schema) = root_schema().await;
        let mut mismatches = Vec::new();
        for args in ["id:\"absent\"", "id:\"root\",expectedVersion:1"] {
            let direct = schema
                .execute(format!(
                    "mutation {{ result:deleteDirectRoot({args}) {{ success error }} }}"
                ))
                .await;
            let opted = schema
                .execute(format!(
                    "mutation {{ result:deleteReviewRoot({args}) {{ success error }} }}"
                ))
                .await;
            if args.contains("expectedVersion") {
                assert_eq!(
                    direct.errors[0].extensions.as_ref().unwrap().get("code"),
                    Some(&async_graphql::Value::from("CONFLICT"))
                );
            } else {
                assert!(direct.errors.is_empty());
                assert_eq!(
                    direct.data.clone().into_json().unwrap()["result"]["error"],
                    "Entity not found"
                );
            }
            if opted.data != direct.data || opted.errors != direct.errors {
                mismatches.push(format!("{args}: opted={opted:?}; direct={direct:?}"));
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM review_root_errors")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn review_extension_equality_cannot_hide_dependents() {
    // String binds as text; citext=text uses different equality from the FK's
    // citext=citext, even though both columns have identical type/collation.
    review_comparison_case("citext", "citext", "P", "p").await;
}
#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn review_blank_padded_equality_refuses() {
    review_comparison_case("CHAR(4)", "CHAR(4)", "P", "P").await;
}

// A DB-only FK has no declared parent type on which the macro can impose a
// marker obligation. Keep the documented opt-out behavior explicit.
mod database_only_parent {
    use super::*;
    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "db_only_parents",
        plural = "DbOnlyParents",
        default_sort = "id ASC"
    )]
    struct DbOnlyParent {
        #[primary_key]
        id: String,
        #[sortable]
        #[filterable(type = "string")]
        name: String,
    }
    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        dependent_deletes,
        table = "db_only_children",
        plural = "DbOnlyChildren",
        default_sort = "id ASC"
    )]
    struct DbOnlyChild {
        #[primary_key]
        id: String,
        #[sortable]
        #[filterable(type = "string")]
        parent_id: String,
    }
    schema_roots! { query_custom_ops: [], entities: [DbOnlyParent,DbOnlyChild], }
    struct DenyChild;
    impl EntityPolicy for DenyChild {
        fn can_access_entity<'a>(
            &'a self,
            _: Option<&'a async_graphql::Context<'_>>,
            _: &'a Database,
            entity: &'static str,
            _: Option<&'static str>,
            kind: EntityAccessKind,
            _: EntityAccessSurface,
        ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
            Box::pin(
                async move { Ok(!(entity == "DbOnlyChild" && kind == EntityAccessKind::Write)) },
            )
        }
    }
    #[tokio::test]
    #[cfg_attr(
        feature = "postgres",
        ignore = "requires test-owned PostgreSQL container"
    )]
    async fn database_only_fk_under_opted_out_parent_still_skips_child_hooks_and_events() {
        #[cfg(feature = "sqlite")]
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        #[cfg(feature = "postgres")]
        let pool = owned_postgres::OwnedPostgres::pool("db-only-delete-policy", 5)
            .await
            .unwrap();
        for sql in [
            "CREATE TABLE db_only_parents(id TEXT PRIMARY KEY,name TEXT NOT NULL)",
            "CREATE TABLE db_only_children(id TEXT PRIMARY KEY,parent_id TEXT NOT NULL REFERENCES db_only_parents(id) ON DELETE CASCADE)",
            "INSERT INTO db_only_parents VALUES('parent','parent')",
            "INSERT INTO db_only_children VALUES('child','parent')",
        ] {
            sqlx::query(sql).execute(&pool).await.unwrap();
        }
        let hook = Hook::default();
        let mut db = Database::with_mutation_hook(pool.clone(), hook.clone());
        db.set_entity_policy(DenyChild);
        let mut events = db
            .ensure_event_sender::<DbOnlyChildChangedEvent>()
            .subscribe();
        let registry = StaticDependentDeletes::<DefaultBackend>::new(
            RuntimeDependentLimits::new(10, 4).unwrap(),
        )
        .register::<DbOnlyChild>()
        .unwrap();
        let schema = schema_builder(db)
            .data(AuthSubject::new("test-principal"))
            .data(registry)
            .finish();
        assert!(
            !schema
                .execute("mutation { deleteDbOnlyChild(id:\"child\") { success } }")
                .await
                .errors
                .is_empty()
        );
        let response = schema
            .execute("mutation { deleteDbOnlyParent(id:\"parent\") { success } }")
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM db_only_children")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            count, 0,
            "opted-out parents retain their physical FK actions"
        );
        assert!(
            hook.0
                .lock()
                .unwrap()
                .iter()
                .all(|(entity, _)| *entity == "DbOnlyParent")
        );
        assert!(events.try_recv().is_err());
    }
}

// Exercise both preparation hooks and After hooks between writes, including
// the final root write. No physical fallback may escape the verified bounds.
struct HookDdl {
    entity: &'static str,
    phase: MutationPhase,
    statements: Vec<&'static str>,
}
#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn hook_ddl_revalidates_catalogue_and_rolls_back_rows_and_events() {
    #[cfg(feature = "sqlite")]
    let cases = vec![
        vec![
            "CREATE TRIGGER hook_cleanup BEFORE DELETE ON delete_parents BEGIN DELETE FROM hook_victims; END",
        ],
        vec![
            "CREATE TEMP TRIGGER hook_cleanup BEFORE DELETE ON main.delete_parents BEGIN DELETE FROM hook_victims; END",
        ],
        vec![
            "CREATE TABLE hook_new(id TEXT PRIMARY KEY,parent_id TEXT REFERENCES delete_parents(id) ON DELETE CASCADE)",
            "INSERT INTO hook_new VALUES('hidden','parent')",
        ],
    ];
    #[cfg(feature = "postgres")]
    let cases = vec![
        vec![
            "CREATE FUNCTION hook_cleanup() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN DELETE FROM hook_victims; RETURN OLD; END $$",
            "CREATE TRIGGER hook_cleanup BEFORE DELETE ON delete_parents FOR EACH ROW EXECUTE FUNCTION hook_cleanup()",
        ],
        vec![
            "ALTER TABLE hook_victims ADD FOREIGN KEY(parent_id) REFERENCES delete_parents(id) ON DELETE CASCADE",
        ],
        vec![
            "CREATE TABLE hook_new(id TEXT PRIMARY KEY,parent_id TEXT REFERENCES delete_parents(id) ON DELETE CASCADE)",
            "INSERT INTO hook_new VALUES('hidden','parent')",
        ],
    ];
    // This benign column addition passes capability/FK validation: only
    // comparison with the verified snapshot can detect it.
    let mut cases = cases;
    cases.push(vec!["ALTER TABLE hook_victims ADD COLUMN extra TEXT"]);
    #[cfg(feature = "postgres")]
    cases.extend([
        vec!["CREATE TABLE hook_inherited () INHERITS (delete_parents)"],
        vec!["CREATE RULE hook_cleanup AS ON DELETE TO delete_parents DO ALSO DELETE FROM hook_victims WHERE parent_id=OLD.id"],
    ]);
    cases.push(vec![
        "CREATE TEMP TABLE delete_parents(id TEXT PRIMARY KEY,name TEXT NOT NULL)",
    ]);
    for statements in cases {
        for (entity, phase) in [
            ("DeleteParent", MutationPhase::Before),
            ("DeleteChild", MutationPhase::Before),
            ("DeleteNull", MutationPhase::After),
            ("DeleteLeaf", MutationPhase::After),
            ("DeleteParent", MutationPhase::After),
        ] {
            // The final parent After hook cannot add a populated FK to a
            // deleted parent. Use an empty source in that case instead.
            let pool = fixture("dependent-hook-ddl").await;
            sqlx::query("CREATE TABLE hook_victims(id TEXT PRIMARY KEY,parent_id TEXT)")
                .execute(&pool)
                .await
                .unwrap();
            if entity != "DeleteParent" || phase != MutationPhase::After {
                sqlx::query("INSERT INTO hook_victims VALUES('victim','parent')")
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            let statements = statements
                .iter()
                .copied()
                .filter(|sql| {
                    !(entity == "DeleteParent"
                        && phase == MutationPhase::After
                        && sql.starts_with("INSERT INTO hook_new"))
                })
                .collect();
            let hook = Hook::default();
            let mut db = Database::with_mutation_hook(pool.clone(), hook.clone());
            db.set_field_policy(Policy {
                deny_entity: false,
                deny_field: false,
            });
            let mut parents = db
                .ensure_event_sender::<DeleteParentChangedEvent>()
                .subscribe();
            let mut children = db
                .ensure_event_sender::<DeleteChildChangedEvent>()
                .subscribe();
            let mut leaves = db
                .ensure_event_sender::<DeleteLeafChangedEvent>()
                .subscribe();
            let mut nulls = db
                .ensure_event_sender::<DeleteNullChangedEvent>()
                .subscribe();
            let notifications = NotificationLog::default();
            let registry = StaticDependentDeletes::<DefaultBackend>::new(
                RuntimeDependentLimits::new(3, 2).unwrap(),
            )
            .register::<DeleteParent>()
            .unwrap()
            .register::<DeleteChild>()
            .unwrap()
            .register::<DeleteLeaf>()
            .unwrap()
            .register::<DeleteNull>()
            .unwrap();
            let response = schema_builder(db)
                .data(AuthSubject::new("test-principal"))
                .data(registry)
                .data(notifications.clone())
                .finish()
                .execute(
                    async_graphql::Request::new(
                        "mutation { deleteDeleteParent(id: \"parent\") { success } }",
                    )
                    .data(HookDdl {
                        entity,
                        phase: phase.clone(),
                        statements,
                    }),
                )
                .await;
            assert_eq!(response.errors.len(), 1, "{entity} {phase:?}: {response:?}");
            assert_eq!(
                response.errors[0].message, "Dependent delete refused",
                "{entity} {phase:?}"
            );
            assert!(
                hook.0.lock().unwrap().contains(&(entity, phase.clone())),
                "DDL hook did not run: {entity} {phase:?}"
            );
            assert_intact(&pool).await;
            assert!(parents.try_recv().is_err());
            assert!(children.try_recv().is_err());
            assert!(leaves.try_recv().is_err());
            assert!(nulls.try_recv().is_err());
            assert!(notifications.0.lock().unwrap().is_empty());
            let victims: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hook_victims")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(
                victims,
                i64::from(entity != "DeleteParent" || phase != MutationPhase::After)
            );
            // A second operation on the same SQLite connection also proves
            // temporary trigger DDL rolled back; all normal writes still work.
            let registry = StaticDependentDeletes::<DefaultBackend>::new(
                RuntimeDependentLimits::new(3, 2).unwrap(),
            )
            .register::<DeleteParent>()
            .unwrap()
            .register::<DeleteChild>()
            .unwrap()
            .register::<DeleteLeaf>()
            .unwrap()
            .register::<DeleteNull>()
            .unwrap();
            let mut retry_db = Database::new(pool.clone());
            retry_db.set_field_policy(Policy {
                deny_entity: false,
                deny_field: false,
            });
            let response = schema_builder(retry_db)
                .data(AuthSubject::new("test-principal"))
                .data(registry)
                .finish()
                .execute("mutation { deleteDeleteParent(id: \"parent\") { success } }")
                .await;
            assert!(
                response.errors.is_empty(),
                "catalogue DDL survived rollback: {response:?}"
            );
        }
    }
}
