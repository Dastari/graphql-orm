//! Hooks and workers present an explicit, named authority to entity, row and
//! field policies (#291). Calls with no authority stay distinguishable.

#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]

use graphql_orm::futures::future::BoxFuture;
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::prelude::*;
use std::sync::{Arc, Mutex};

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    backend = "sqlite",
    table = "authority_orders",
    plural = "AuthorityOrders"
)]
struct AuthorityOrder {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    label: String,
}

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    backend = "sqlite",
    table = "authority_lines",
    plural = "AuthorityLines"
)]
struct AuthorityLine {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    order_id: String,
    note: String,
}

schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [AuthorityOrder, AuthorityLine] }

/// What each policy was asked, and under which named access.
type Asked = Arc<Mutex<Vec<(&'static str, &'static str, Option<String>)>>>;

/// A GraphQL request is admitted. A repository-surface call is admitted only
/// when the handle presents one of the named system accesses this host knows.
struct NamedAccess(Asked);
impl NamedAccess {
    fn decide(
        &self,
        callback: &'static str,
        ctx: Option<&async_graphql::Context<'_>>,
        db: &Database,
        entity: &'static str,
        surface: EntityAccessSurface,
    ) -> bool {
        if ctx.is_some() {
            return true;
        }
        assert_eq!(surface, EntityAccessSurface::Repository);
        let access = db.system_access();
        self.0
            .lock()
            .unwrap()
            .push((callback, entity, access.map(|access| access.name.clone())));
        access.is_some_and(|access| {
            access.name == "line-cleanup" && entity == "AuthorityLine"
                || access.name == "billing-worker" && access.has_capability("lines.write")
        })
    }
}
impl EntityPolicy for NamedAccess {
    fn can_access_entity<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(self.decide("entity", ctx, db, entity, surface)) })
    }
}
impl RowPolicy for NamedAccess {
    fn can_read_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        surface: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(self.decide("row-read", ctx, db, entity, surface)) })
    }
    fn can_write_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        surface: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(self.decide("row-write", ctx, db, entity, surface)) })
    }
}
impl FieldPolicy for NamedAccess {
    fn can_read_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
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
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn can_read_repository_field<'a>(
        &'a self,
        _: Option<AccessContext<'a>>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn can_write_repository_field<'a>(
        &'a self,
        access: Option<AccessContext<'a>>,
        db: &'a Database,
        entity: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            // The handle's access and the callback's access are the same one.
            let named = access
                .and_then(AccessContext::system)
                .map(|access| access.name.clone());
            assert_eq!(named, db.system_access().map(|access| access.name.clone()));
            self.0.lock().unwrap().push(("field-write", entity, named));
            Ok(true)
        })
    }
}

fn lines_of(order: &str) -> AuthorityLineWhereInput {
    AuthorityLineWhereInput {
        order_id: Some(graphql_orm::graphql::filters::StringFilter {
            eq: Some(order.into()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Deletes an order's lines before the order itself is deleted.
struct CleanupHook {
    scoped: bool,
}
impl MutationHook for CleanupHook {
    fn on_mutation<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        hook_ctx: &'a mut MutationContext<'_>,
        event: &'a MutationEvent,
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if event.entity_name != "AuthorityOrder"
                || event.action != ChangeAction::Deleted
                || event.phase != MutationPhase::Before
            {
                return Ok(());
            }
            // The hook was given the originating request: it decides from it
            // whether to act with its own named authority.
            assert!(ctx.is_some(), "the order is deleted through GraphQL");
            let order = event.id.clone();
            if self.scoped {
                hook_ctx
                    .with_system_access(SystemAccess::new("line-cleanup"), |tx| {
                        Box::pin(async move {
                            assert_eq!(
                                tx.database().system_access().map(|a| a.name.as_str()),
                                Some("line-cleanup")
                            );
                            tx.delete_where::<AuthorityLine>(lines_of(&order)).await
                        })
                    })
                    .await
                    .map_err(graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                // The scope ended: later writes present no authority again.
                assert!(hook_ctx.database().system_access().is_none());
            } else {
                hook_ctx
                    .delete_where::<AuthorityLine>(lines_of(&order))
                    .await
                    .map_err(graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
            }
            Ok(())
        })
    }
}

async fn setup(scoped_hook: bool) -> (Database, Asked) {
    let mut db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    for ddl in [
        "CREATE TABLE authority_orders (id TEXT PRIMARY KEY, label TEXT NOT NULL)",
        "CREATE TABLE authority_lines (id TEXT PRIMARY KEY, order_id TEXT NOT NULL, note TEXT NOT NULL)",
        "INSERT INTO authority_orders VALUES ('o1', 'first')",
        "INSERT INTO authority_lines VALUES ('l1', 'o1', 'a'), ('l2', 'o1', 'b'), ('l3', 'o2', 'c')",
    ] {
        graphql_orm::sqlx::query(ddl)
            .execute(db.pool())
            .await
            .unwrap();
    }
    let asked: Asked = Default::default();
    db.set_entity_policy(NamedAccess(asked.clone()));
    db.set_row_policy(NamedAccess(asked.clone()));
    db.set_field_policy(NamedAccess(asked.clone()));
    db.set_mutation_hook(CleanupHook {
        scoped: scoped_hook,
    });
    (db, asked)
}

async fn line_count(db: &Database) -> i64 {
    let count: (i64,) = graphql_orm::sqlx::query_as("SELECT COUNT(*) FROM authority_lines")
        .fetch_one(db.pool())
        .await
        .unwrap();
    count.0
}

async fn delete_order(db: &Database) -> async_graphql::Response {
    schema_builder(db.clone())
        .data(AuthSubject::new("user-1"))
        .finish()
        .execute(r#"mutation { deleteAuthorityOrder(id: "o1") { success error } }"#)
        .await
}

fn new_line(id: &str) -> CreateAuthorityLineInput {
    CreateAuthorityLineInput {
        id: id.into(),
        order_id: "o9".into(),
        note: "n".into(),
    }
}

#[tokio::test]
async fn a_hook_write_without_authority_stays_refused_and_distinguishable() {
    let (db, asked) = setup(false).await;
    let response = delete_order(&db).await;
    let refused = !response.errors.is_empty()
        || response.data.into_json().unwrap()["deleteAuthorityOrder"]["success"] == false;
    assert!(refused, "the hook's cleanup had no authority");
    assert_eq!(line_count(&db).await, 3, "nothing was deleted");
    // The policy saw a repository call that presented nothing.
    let asked = asked.lock().unwrap();
    assert!(
        asked
            .iter()
            .any(|(callback, entity, access)| *callback == "entity"
                && *entity == "AuthorityLine"
                && access.is_none()),
        "{asked:?}"
    );
}

#[tokio::test]
async fn a_hook_scope_presents_its_named_access_to_entity_and_row_policies() {
    let (db, asked) = setup(true).await;
    let response = delete_order(&db).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap()["deleteAuthorityOrder"]["success"],
        true
    );
    assert_eq!(line_count(&db).await, 1, "the order's two lines are gone");
    let asked = asked.lock().unwrap();
    for callback in ["entity", "row-write"] {
        assert!(
            asked.iter().any(|(asked_callback, entity, access)| {
                *asked_callback == callback
                    && *entity == "AuthorityLine"
                    && access.as_deref() == Some("line-cleanup")
            }),
            "{callback}: {asked:?}"
        );
    }
    // The scope names one entity's cleanup; it is not a general grant.
    assert!(
        asked
            .iter()
            .all(|(_, _, access)| access.as_deref() != Some("billing-worker"))
    );
}

#[tokio::test]
async fn a_worker_handle_presents_a_named_system_access() {
    let (db, asked) = setup(true).await;

    // No authority: refused, as before.
    let error = AuthorityLine::insert(&db, new_line("w0"))
        .await
        .unwrap_err();
    assert_eq!(
        OrmPublicError::from_sqlx(&error).code,
        OrmErrorCode::Forbidden
    );

    // A named access the policy does not admit is refused too.
    let unknown = db
        .clone()
        .with_system_access(SystemAccess::new("unknown-job"));
    assert!(
        AuthorityLine::insert(&unknown, new_line("w1"))
            .await
            .is_err()
    );
    // The right name without the capability the policy asks for, likewise.
    let weak = db
        .clone()
        .with_system_access(SystemAccess::new("billing-worker"));
    assert!(AuthorityLine::insert(&weak, new_line("w2")).await.is_err());

    let worker = db.clone().with_system_access(
        SystemAccess::new("billing-worker").with_capabilities(vec!["lines.write".into()]),
    );
    assert_eq!(
        worker.system_access().map(|access| access.name.as_str()),
        Some("billing-worker")
    );
    asked.lock().unwrap().clear();
    AuthorityLine::insert(&worker, new_line("w3"))
        .await
        .unwrap();
    {
        let asked = asked.lock().unwrap();
        for callback in ["entity", "row-write", "field-write"] {
            assert!(
                asked.iter().any(|(asked_callback, _, access)| {
                    *asked_callback == callback && access.as_deref() == Some("billing-worker")
                }),
                "{callback}: {asked:?}"
            );
        }
    }

    // A transaction on the labelled handle carries it for every write.
    worker
        .transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                tx.insert::<AuthorityLine>(new_line("w4"))
                    .await
                    .map_err(OrmPublicError::from)?;
                Ok(())
            })
        })
        .await
        .unwrap();
    assert_eq!(line_count(&db).await, 5);

    // The label is the handle's: the original handle and an unlabelled copy
    // present nothing.
    assert!(db.system_access().is_none());
    assert!(
        worker
            .clone()
            .without_system_access()
            .system_access()
            .is_none()
    );
    assert!(AuthorityLine::insert(&db, new_line("w5")).await.is_err());
}

#[tokio::test]
async fn an_abandoned_scope_cannot_commit() {
    let (db, _) = setup(true).await;
    let worker = db.clone().with_system_access(
        SystemAccess::new("billing-worker").with_capabilities(vec!["lines.write".into()]),
    );
    let outcome = worker
        .transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                tx.insert::<AuthorityLine>(new_line("a1"))
                    .await
                    .map_err(OrmPublicError::from)?;
                // The scope's future is dropped before it completes.
                let scope = tx.with_system_access(SystemAccess::new("line-cleanup"), |_| {
                    Box::pin(graphql_orm::futures::future::pending::<()>())
                });
                assert!(
                    tokio::time::timeout(std::time::Duration::from_millis(20), scope)
                        .await
                        .is_err()
                );
                Ok(())
            })
        })
        .await;
    match outcome {
        Err(TransactionError::Rejected(error)) => assert_eq!(error.code, OrmErrorCode::Conflict),
        other => panic!("an abandoned scope must not commit: {other:?}"),
    }
    assert_eq!(line_count(&db).await, 3, "the transaction rolled back");
}
