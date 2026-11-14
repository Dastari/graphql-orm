//! Scoped system access and transaction extensions on every backend's
//! generated write templates (#291, #564): every policy check of a write made
//! inside a `with_system_access` scope, the written-row check included, runs
//! under the scope's authority.

// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]

#[cfg(feature = "mssql")]
#[allow(dead_code)]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(backend = "sqlite", table = "scoped_records", plural = "ScopedRecords")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "scoped_records",
        plural = "ScopedRecords"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "scoped_records",
        plural = "ScopedRecords",
        schema_policy = "external_writable"
    )
)]
struct ScopedRecord {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    #[filterable(type = "string")]
    id: String,
    #[filterable(type = "string")]
    zone: String,
    note: String,
}

#[cfg(feature = "sqlite")]
schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [ScopedRecord] }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [ScopedRecord] }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [ScopedRecord] }

#[cfg(feature = "sqlite")]
struct FixtureOwner;
#[cfg(feature = "sqlite")]
impl FixtureOwner {
    fn cleanup(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }
}
#[cfg(feature = "postgres")]
type FixtureOwner = owned_postgres::OwnedPostgres;
#[cfg(feature = "mssql")]
type FixtureOwner = owned_mssql::OwnedSqlServer;

struct AllowEntityPolicy;
impl EntityPolicy for AllowEntityPolicy {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

/// Row authority by named access:
/// - a GraphQL request may write any row;
/// - `admin` may write any row;
/// - `zone-a` may write only rows whose zone is `a`;
/// - a repository call with no access may write nothing.
struct ZoneRows;
impl ZoneRows {
    fn may_write(
        ctx: Option<&async_graphql::Context<'_>>,
        db: &Database,
        row: &(dyn std::any::Any + Send + Sync),
    ) -> bool {
        if ctx.is_some() {
            return true;
        }
        let row = row.downcast_ref::<ScopedRecord>().expect("a ScopedRecord");
        match db.system_access().map(|access| access.name.as_str()) {
            Some("admin") => true,
            Some("zone-a") => row.zone == "a",
            _ => false,
        }
    }
}
impl RowPolicy for ZoneRows {
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn can_write_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(Self::may_write(ctx, db, row)) })
    }
}

/// The capability a transaction's owner attaches for its hook.
struct Reviewed {
    dropped: Arc<AtomicBool>,
}
impl Drop for Reviewed {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

/// Records whether each write's hook saw the extension, and writes an audit
/// row in its own named scope when a GraphQL create asks for one.
struct ScopeHook(Arc<Mutex<Vec<bool>>>);
impl MutationHook for ScopeHook {
    fn on_mutation<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        hook_ctx: &'a mut MutationContext<'_>,
        event: &'a MutationEvent,
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if event.phase != MutationPhase::After {
                return Ok(());
            }
            self.0
                .lock()
                .unwrap()
                .push(hook_ctx.extension::<Reviewed>().is_some());
            let created = event.after::<ScopedRecord>()?;
            if ctx.is_some()
                && event.action == ChangeAction::Created
                && created.is_some_and(|row| row.note == "with-audit")
            {
                let id = format!("audit-{}", event.id);
                hook_ctx
                    .with_system_access(SystemAccess::new("zone-a"), |tx| {
                        Box::pin(async move { tx.insert::<ScopedRecord>(record(&id, "a")).await })
                    })
                    .await
                    .map_err(graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
            }
            Ok(())
        })
    }
}

fn record(id: &str, zone: &str) -> CreateScopedRecordInput {
    CreateScopedRecordInput {
        id: id.into(),
        zone: zone.into(),
        note: "n".into(),
    }
}

fn move_to(zone: &str) -> UpdateScopedRecordInput {
    UpdateScopedRecordInput {
        zone: Some(zone.into()),
        ..Default::default()
    }
}

async fn setup() -> (Database, FixtureOwner, Arc<Mutex<Vec<bool>>>) {
    #[cfg(feature = "sqlite")]
    let (mut db, owner) = (
        Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
            .await
            .unwrap(),
        FixtureOwner,
    );
    #[cfg(feature = "postgres")]
    let (mut db, owner) = {
        let owner = FixtureOwner::start("transaction-scope").unwrap();
        let pool = graphql_orm::sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&owner.url)
            .await
            .unwrap();
        (Database::new(pool), owner)
    };
    #[cfg(feature = "mssql")]
    let (mut db, owner) = {
        let owner = FixtureOwner::start().await.unwrap();
        let pool = graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(
            &owner.connection_string,
        )
        .await
        .unwrap();
        (
            Database::builder(pool)
                .schema_policy(SchemaPolicy::ExternalWritable)
                .build(),
            owner,
        )
    };
    #[cfg(not(feature = "mssql"))]
    let ddl =
        "CREATE TABLE scoped_records (id TEXT PRIMARY KEY, zone TEXT NOT NULL, note TEXT NOT NULL)";
    #[cfg(feature = "mssql")]
    let ddl = "CREATE TABLE scoped_records (id NVARCHAR(80) PRIMARY KEY, zone NVARCHAR(40) NOT NULL, note NVARCHAR(40) NOT NULL)";
    for sql in [
        ddl,
        "INSERT INTO scoped_records VALUES ('r1', 'a', 'n')",
        "INSERT INTO scoped_records VALUES ('r2', 'a', 'n')",
    ] {
        <DefaultBackend as WriteBackend>::execute_with_binds(db.pool(), sql, &[])
            .await
            .unwrap();
    }
    let seen = Arc::new(Mutex::new(Vec::new()));
    db.set_entity_policy(AllowEntityPolicy);
    db.set_row_policy(ZoneRows);
    db.set_mutation_hook(ScopeHook(seen.clone()));
    (db, owner, seen)
}

async fn stored(db: &Database) -> Vec<(String, String)> {
    DefaultBackend::fetch_rows(
        db.pool(),
        "SELECT id, zone FROM scoped_records ORDER BY id",
        &[],
    )
    .await
    .unwrap()
    .iter()
    .map(|row| {
        (
            DefaultBackend::try_get_string(row, "id").unwrap(),
            DefaultBackend::try_get_string(row, "zone").unwrap(),
        )
    })
    .collect()
}

fn pairs(rows: &[(&str, &str)]) -> Vec<(String, String)> {
    rows.iter()
        .map(|(id, zone)| ((*id).to_owned(), (*zone).to_owned()))
        .collect()
}

fn admin(db: &Database) -> Database {
    db.clone().with_system_access(SystemAccess::new("admin"))
}

fn rejected_code(outcome: Result<(), TransactionError>) -> OrmErrorCode {
    match outcome {
        Err(TransactionError::Rejected(error)) => error.code,
        other => panic!("expected a rejected transaction: {other:?}"),
    }
}

/// A broadly privileged handle that enters a restricted scope writes under
/// the restriction: the written row is checked with the scope's authority.
#[tokio::test]
async fn a_scoped_insert_is_checked_under_the_scope_before_and_after_the_write() {
    let (db, mut owner, _) = setup().await;
    let admin = admin(&db);

    // Allowed by the scope: committed.
    admin
        .transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                tx.with_system_access(SystemAccess::new("zone-a"), |tx| {
                    Box::pin(async move { tx.insert::<ScopedRecord>(record("in-a", "a")).await })
                })
                .await
                .map_err(OrmPublicError::from)?;
                // Outside the scope the handle's own authority applies again.
                tx.insert::<ScopedRecord>(record("in-b", "b"))
                    .await
                    .map_err(OrmPublicError::from)?;
                Ok(())
            })
        })
        .await
        .unwrap();

    // Refused by the scope although the base handle could write it. The
    // refusal poisons the transaction even when the caller ignores it, and
    // the earlier unscoped write rolls back with it.
    let outcome = admin
        .transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                tx.insert::<ScopedRecord>(record("before", "b"))
                    .await
                    .map_err(OrmPublicError::from)?;
                let refused = tx
                    .with_system_access(SystemAccess::new("zone-a"), |tx| {
                        Box::pin(
                            async move { tx.insert::<ScopedRecord>(record("out-b", "b")).await },
                        )
                    })
                    .await
                    .expect_err("zone-a may not create a zone b row");
                assert_eq!(
                    OrmPublicError::from_sqlx(&refused).code,
                    OrmErrorCode::Forbidden
                );
                Ok(())
            })
        })
        .await;
    assert_eq!(rejected_code(outcome), OrmErrorCode::Forbidden);
    assert_eq!(
        stored(&db).await,
        pairs(&[("in-a", "a"), ("in-b", "b"), ("r1", "a"), ("r2", "a")])
    );
    owner.cleanup().unwrap();
}

#[tokio::test]
async fn a_scoped_update_cannot_move_a_row_out_of_the_scope() {
    let (db, mut owner, _) = setup().await;
    let admin = admin(&db);

    // Within the scope: a permitted row stays permitted.
    admin
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                tx.with_system_access(SystemAccess::new("zone-a"), |tx| {
                    Box::pin(async move {
                        tx.update_by_id::<ScopedRecord>(
                            &"r1".to_owned(),
                            UpdateScopedRecordInput {
                                note: Some("kept".into()),
                                ..Default::default()
                            },
                        )
                        .await
                    })
                })
                .await
                .map_err(OrmPublicError::from)?;
                Ok(())
            })
        })
        .await
        .unwrap();

    // The existing row is permitted, the row as written is not: single-row
    // and bulk updates are both refused and rolled back.
    for bulk in [false, true] {
        let outcome = admin
            .transaction(TransactionMode::StateMachine, |tx| {
                Box::pin(async move {
                    let refused = tx
                        .with_system_access(SystemAccess::new("zone-a"), |tx| {
                            Box::pin(async move {
                                if bulk {
                                    tx.update_where::<ScopedRecord>(
                                        ScopedRecordWhereInput {
                                            id: Some(graphql_orm::graphql::filters::StringFilter {
                                                eq: Some("r2".into()),
                                                ..Default::default()
                                            }),
                                            ..Default::default()
                                        },
                                        move_to("b"),
                                    )
                                    .await
                                    .map(|_| ())
                                } else {
                                    tx.update_by_id::<ScopedRecord>(&"r2".to_owned(), move_to("b"))
                                        .await
                                        .map(|_| ())
                                }
                            })
                        })
                        .await
                        .expect_err("zone-a may not move a row to zone b");
                    assert_eq!(
                        OrmPublicError::from_sqlx(&refused).code,
                        OrmErrorCode::Forbidden,
                        "bulk={bulk}"
                    );
                    Ok(())
                })
            })
            .await;
        assert_eq!(
            rejected_code(outcome),
            OrmErrorCode::Forbidden,
            "bulk={bulk}"
        );
        assert_eq!(stored(&db).await, pairs(&[("r1", "a"), ("r2", "a")]));
    }

    // The unscoped handle may still do it.
    ScopedRecord::update_by_id(&admin, &"r2".to_owned(), move_to("b"))
        .await
        .unwrap();
    assert_eq!(stored(&db).await, pairs(&[("r1", "a"), ("r2", "b")]));
    owner.cleanup().unwrap();
}

/// A hook on an unlabelled handle writes in its own named scope: every check
/// of that nested write, the written-row check included, sees the scope.
#[tokio::test]
async fn a_hook_scope_carries_its_access_through_the_written_row_check() {
    let (db, mut owner, _) = setup().await;
    // Without a scope the same repository write has no authority.
    assert!(
        ScopedRecord::insert(&db, record("plain", "a"))
            .await
            .is_err()
    );
    let response = schema_builder(db.clone())
        .data(AuthSubject::new("user-1"))
        .finish()
        .execute(
            r#"mutation { createScopedRecord(input: {id: "g1", zone: "b", note: "with-audit"}) { success } }"#,
        )
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        stored(&db).await,
        pairs(&[("audit-g1", "a"), ("g1", "b"), ("r1", "a"), ("r2", "a")])
    );
    owner.cleanup().unwrap();
}

/// Transaction extensions on this backend's templates: single, bulk and
/// conditional writes see the extension, and it ends before the commit.
#[tokio::test]
async fn hooks_read_the_transaction_extension_on_every_generated_write() {
    let (db, mut owner, seen) = setup().await;
    let admin = admin(&db);
    let dropped = Arc::new(AtomicBool::new(false));
    let extension = Reviewed {
        dropped: dropped.clone(),
    };
    let dropped_before_commit = Arc::new(Mutex::new(None));
    let observed = dropped_before_commit.clone();
    let at_commit = dropped.clone();
    admin
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                tx.insert_extension(extension)?;
                tx.insert::<ScopedRecord>(record("e1", "a"))
                    .await
                    .map_err(OrmPublicError::from)?;
                tx.update_by_id::<ScopedRecord>(&"r1".to_owned(), move_to("b"))
                    .await
                    .map_err(OrmPublicError::from)?;
                tx.update_where::<ScopedRecord>(
                    ScopedRecordWhereInput {
                        zone: Some(graphql_orm::graphql::filters::StringFilter {
                            eq: Some("a".into()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    move_to("c"),
                )
                .await
                .map_err(OrmPublicError::from)?;
                tx.delete_by_id::<ScopedRecord>(&"e1".to_owned())
                    .await
                    .map_err(OrmPublicError::from)?;
                tx.defer(move |_| async move {
                    *observed.lock().unwrap() = Some(at_commit.load(Ordering::SeqCst));
                    Ok::<(), String>(())
                });
                Ok(())
            })
        })
        .await
        .unwrap();
    // insert, update, two bulk rows, delete.
    assert_eq!(*seen.lock().unwrap(), [true; 5]);
    assert_eq!(*dropped_before_commit.lock().unwrap(), Some(true));
    assert_eq!(stored(&db).await, pairs(&[("r1", "b"), ("r2", "c")]));

    // The next transaction on the same handle starts with none.
    seen.lock().unwrap().clear();
    ScopedRecord::update_by_id(&admin, &"r1".to_owned(), move_to("a"))
        .await
        .unwrap();
    assert_eq!(*seen.lock().unwrap(), [false]);
    owner.cleanup().unwrap();
}
