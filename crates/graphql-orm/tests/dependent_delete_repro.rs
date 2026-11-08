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
    notify_with = "Self::on_committed",
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
    async fn on_committed(
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
    assert_eq!(response.errors[0].message, "Dependent delete refused");
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
