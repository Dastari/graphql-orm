#![cfg(all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))))]
//! Physical-relation regressions from the independent PR #388 recheck.
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
use graphql_orm::prelude::*;
use std::sync::{Arc, Mutex};

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    dependent_deletes,
    table = "physical_roots",
    plural = "PhysicalRoots",
    default_sort = "id ASC"
)]
struct PhysicalRoot {
    #[primary_key]
    id: String,
    #[sortable]
    #[filterable(type = "string")]
    name: String,
}
#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    dependent_deletes,
    table = "physical_victims",
    plural = "PhysicalVictims",
    default_sort = "id ASC"
)]
struct PhysicalVictim {
    #[primary_key]
    id: String,
    #[sortable]
    #[filterable(type = "string")]
    root_ref: String,
}
schema_roots! { query_custom_ops: [], entities: [PhysicalRoot,PhysicalVictim], }
#[derive(Clone, Default)]
struct Hooks(Arc<Mutex<Vec<String>>>);
struct InstallRule;
const RULE: &str = "CREATE RULE physical_cleanup AS ON DELETE TO physical_roots DO ALSO DELETE FROM physical_victims WHERE root_ref=OLD.id";
impl MutationHook for Hooks {
    fn on_mutation<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        tx: &'a mut MutationContext<'_>,
        event: &'a MutationEvent,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if event.entity_name == "PhysicalRoot"
                && event.phase == MutationPhase::Before
                && ctx.is_some_and(|ctx| ctx.data_opt::<InstallRule>().is_some())
            {
                tx.execute(RULE, &[])
                    .await
                    .map_err(graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
            }
            self.0
                .lock()
                .unwrap()
                .push(format!("{}:{:?}", event.entity_name, event.phase));
            Ok(())
        })
    }
}
struct DenyVictim;
impl EntityPolicy for DenyVictim {
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
            async move { Ok(!(entity == "PhysicalVictim" && kind == EntityAccessKind::Write)) },
        )
    }
}
async fn refuse(statements: &[&str], late_rule: bool) {
    let pool = owned_postgres::OwnedPostgres::pool("dependent-physical-relations", 5)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE physical_victims(id TEXT PRIMARY KEY,root_ref TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    for sql in statements {
        sqlx::query(sql)
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    sqlx::query("INSERT INTO physical_victims VALUES('victim','root')")
        .execute(&pool)
        .await
        .unwrap();
    let hooks = Hooks::default();
    let mut db = Database::with_mutation_hook(pool.clone(), hooks.clone());
    db.set_entity_policy(DenyVictim);
    let mut events = db
        .ensure_event_sender::<PhysicalVictimChangedEvent>()
        .subscribe();
    let mut roots = db
        .ensure_event_sender::<PhysicalRootChangedEvent>()
        .subscribe();
    let registry =
        StaticDependentDeletes::<DefaultBackend>::new(RuntimeDependentLimits::new(10, 4).unwrap())
            .register::<PhysicalRoot>()
            .unwrap()
            .register::<PhysicalVictim>()
            .unwrap();
    let schema = schema_builder(db)
        .data(AuthSubject::new("test-principal"))
        .data(registry)
        .finish();
    assert!(
        !schema
            .execute("mutation { deletePhysicalVictim(id:\"victim\") { success } }")
            .await
            .errors
            .is_empty()
    );
    let request =
        async_graphql::Request::new("mutation { deletePhysicalRoot(id:\"root\") { success } }");
    let request = if late_rule {
        request.data(InstallRule)
    } else {
        request
    };
    let response = schema.execute(request).await;
    let victims: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM physical_victims")
        .fetch_one(&pool)
        .await
        .unwrap();
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM physical_roots")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        !response.errors.is_empty(),
        "unmodeled physical writes were accepted: {:?}; victims={victims}; roots={remaining}; hooks={:?}",
        response.data,
        hooks.0.lock().unwrap()
    );
    assert_eq!(response.errors[0].message, "Dependent delete refused");
    assert_eq!(victims, 1);
    assert_eq!(remaining, 1);
    assert!(events.try_recv().is_err());
    assert!(roots.try_recv().is_err());
    if !late_rule {
        assert!(hooks.0.lock().unwrap().is_empty());
    } else {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM pg_rewrite WHERE rulename='physical_cleanup'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 0, "hook DDL must roll back with the refused delete");
    }
}
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn inheritance_descendants_cannot_hide_foreign_keys() {
    refuse(&[
        "CREATE TABLE physical_roots(id TEXT PRIMARY KEY,name TEXT NOT NULL)",
        "CREATE TABLE physical_extra () INHERITS (physical_roots)",
        "ALTER TABLE physical_extra ADD PRIMARY KEY(id)",
        "ALTER TABLE physical_victims ADD FOREIGN KEY(root_ref) REFERENCES physical_extra(id) ON DELETE CASCADE",
        "INSERT INTO physical_extra VALUES('root','root')",
    ],false).await;
}
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn registered_inheritance_child_refuses() {
    refuse(
        &[
            "CREATE TABLE physical_base(id TEXT PRIMARY KEY,name TEXT NOT NULL)",
            "CREATE TABLE physical_roots () INHERITS (physical_base)",
            "ALTER TABLE physical_roots ADD PRIMARY KEY(id)",
            "INSERT INTO physical_roots VALUES('root','root')",
        ],
        false,
    )
    .await;
}
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn partitions_cannot_hide_foreign_keys() {
    refuse(&[
        "CREATE TABLE physical_roots(id TEXT PRIMARY KEY,name TEXT NOT NULL) PARTITION BY LIST(id)",
        "CREATE TABLE physical_part PARTITION OF physical_roots FOR VALUES IN ('root')",
        "ALTER TABLE physical_victims ADD FOREIGN KEY(root_ref) REFERENCES physical_part(id) ON DELETE CASCADE",
        "INSERT INTO physical_roots VALUES('root','root')",
    ],false).await;
}
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn registered_partition_leaf_refuses() {
    refuse(&[
        "CREATE TABLE physical_base(id TEXT PRIMARY KEY,name TEXT NOT NULL) PARTITION BY LIST(id)",
        "CREATE TABLE physical_roots PARTITION OF physical_base FOR VALUES IN ('root')",
        "INSERT INTO physical_roots VALUES('root','root')",
    ],false).await;
}
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn rewrite_rules_cannot_delete_unauthorized_entities() {
    refuse(
        &[
            "CREATE TABLE physical_roots(id TEXT PRIMARY KEY,name TEXT NOT NULL)",
            RULE,
            "INSERT INTO physical_roots VALUES('root','root')",
        ],
        false,
    )
    .await;
}
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn rewrite_rules_installed_by_hooks_are_rechecked() {
    refuse(
        &[
            "CREATE TABLE physical_roots(id TEXT PRIMARY KEY,name TEXT NOT NULL)",
            "INSERT INTO physical_roots VALUES('root','root')",
        ],
        true,
    )
    .await;
}
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn registered_view_refuses() {
    refuse(
        &[
            "CREATE TABLE physical_base(id TEXT PRIMARY KEY,name TEXT NOT NULL)",
            "INSERT INTO physical_base VALUES('root','root')",
            "CREATE VIEW physical_roots AS SELECT * FROM physical_base",
        ],
        false,
    )
    .await;
}
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn registered_foreign_table_refuses() {
    refuse(&[
        "CREATE EXTENSION file_fdw",
        "CREATE SERVER physical_files FOREIGN DATA WRAPPER file_fdw",
        "COPY (SELECT 'root'::text,'root'::text) TO '/tmp/physical-root.csv' WITH CSV",
        "CREATE FOREIGN TABLE physical_roots(id TEXT,name TEXT) SERVER physical_files OPTIONS(filename '/tmp/physical-root.csv',format 'csv')",
    ],false).await;
}
