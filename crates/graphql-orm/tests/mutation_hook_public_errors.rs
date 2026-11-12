// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(any(feature = "sqlite", feature = "postgres"))]

//! A mutation hook's deliberate public refusal reaches repository callers with
//! its code and message, in both hook phases and on every write method, while
//! any other hook failure stays a sanitized internal error.

#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use graphql_orm::futures::future::BoxFuture;
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::prelude::*;
use std::sync::{Arc, Mutex};

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "hook_records",
        plural = "HookRecords",
        upsert = "record_key"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "hook_records",
        plural = "HookRecords",
        upsert = "record_key"
    )
)]
struct HookRecord {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[unique]
    #[sortable]
    #[filterable(type = "string")]
    record_key: String,
    #[filterable(type = "string")]
    status: String,
    #[graphql_orm(version, default = "0")]
    #[filterable(type = "number")]
    #[sortable]
    version: i64,
}

/// Composite-key entity for the key-addressed repository writes.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "hook_grants",
        plural = "HookGrants",
        repository_mutations = true,
        unique_composite = "subject_id,grant_name",
        default_sort = "subject_id ASC, grant_name ASC",
        upsert = "subject_id,grant_name"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "hook_grants",
        plural = "HookGrants",
        repository_mutations = true,
        unique_composite = "subject_id,grant_name",
        default_sort = "subject_id ASC, grant_name ASC",
        upsert = "subject_id,grant_name"
    )
)]
struct HookGrant {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    subject_id: String,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    grant_name: String,
    #[filterable(type = "string")]
    status: String,
}

/// Append-only entity for the hooks that retention purge runs.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "hook_retained",
        plural = "HookRetaineds",
        append_only = true,
        retention_purge = "hook_retained.purge"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "hook_retained",
        plural = "HookRetaineds",
        append_only = true,
        retention_purge = "hook_retained.purge"
    )
)]
struct HookRetained {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    #[sortable]
    kind: String,
}

#[cfg(feature = "sqlite")]
schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [HookRecord, HookGrant] }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [HookRecord, HookGrant] }

const SECRET: &str = "SELECT secret FROM private_table";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Failure {
    /// A deliberate public refusal with an allow-listed code and safe message.
    Public(OrmErrorCode),
    /// The same typed refusal returned through `?` instead of `into_graphql_error`.
    PublicPropagated(OrmErrorCode),
    /// An authored error without a `code` extension.
    Uncoded,
    /// An authored error whose `code` extension is not an ORM public code.
    UnknownCode,
    /// A driver failure propagated through `?`.
    Propagated,
}

impl Failure {
    fn error(self) -> async_graphql::Error {
        use async_graphql::ErrorExtensions;
        match self {
            Self::Public(code) => {
                OrmPublicError::with_message(code, "refused by hook policy").into_graphql_error()
            }
            Self::PublicPropagated(code) => {
                let refused: async_graphql::Result<()> = (|| {
                    Err(OrmPublicError::with_message(code, "refused by hook policy"))?;
                    Ok(())
                })();
                refused.unwrap_err()
            }
            Self::Uncoded => async_graphql::Error::new(SECRET),
            Self::UnknownCode => {
                async_graphql::Error::new(SECRET).extend_with(|_, ext| ext.set("code", "PRIVATE"))
            }
            Self::Propagated => {
                async_graphql::Error::from(graphql_orm::sqlx::Error::Protocol(SECRET.into()))
            }
        }
    }
}

#[derive(Clone)]
struct RefusingHook {
    phase: Arc<Mutex<Option<(MutationPhase, Failure)>>>,
}

impl MutationHook for RefusingHook {
    fn on_mutation<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a mut MutationContext<'_>,
        event: &'a MutationEvent,
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        let armed = self.phase.lock().unwrap().clone();
        Box::pin(async move {
            match armed {
                Some((phase, failure)) if phase == event.phase => Err(failure.error()),
                _ => Ok(()),
            }
        })
    }
}

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

struct DenyWrites;
impl EntityPolicy for DenyWrites {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        kind: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(kind != EntityAccessKind::Write) })
    }
}

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

async fn fixture() -> (Database, FixtureOwner, RefusingHook) {
    #[cfg(feature = "sqlite")]
    let (mut db, owner) = (
        Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
            .await
            .unwrap(),
        FixtureOwner,
    );
    #[cfg(feature = "postgres")]
    let (mut db, owner) = {
        let owner = FixtureOwner::start("mutation-hook-public-errors").unwrap();
        let pool = graphql_orm::sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&owner.url)
            .await
            .unwrap();
        (Database::new(pool), owner)
    };
    let plan = db
        .schema()
        .plan_migration_to_entities(
            "hook-errors",
            "hook errors",
            &[
                HookRecord::metadata(),
                HookGrant::metadata(),
                HookRetained::metadata(),
            ],
        )
        .await
        .unwrap();
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .unwrap();
    let hook = RefusingHook {
        phase: Arc::new(Mutex::new(None)),
    };
    db.set_entity_policy(AllowEntityPolicy);
    db.set_mutation_hook(hook.clone());
    (db, owner, hook)
}

fn create(key: &str) -> CreateHookRecordInput {
    CreateHookRecordInput {
        record_key: key.to_string(),
        status: "before".to_string(),
    }
}

fn update() -> UpdateHookRecordInput {
    UpdateHookRecordInput {
        record_key: None,
        status: Some("after".to_string()),
    }
}

fn grant(grant_name: &str) -> CreateHookGrantInput {
    CreateHookGrantInput {
        subject_id: "subject".to_string(),
        grant_name: grant_name.to_string(),
        status: "before".to_string(),
    }
}

fn grant_key() -> HookGrantKey {
    HookGrantKey {
        subject_id: "subject".to_string(),
        grant_name: "existing".to_string(),
    }
}

fn grant_update() -> UpdateHookGrantInput {
    UpdateHookGrantInput {
        status: Some("after".to_string()),
    }
}

fn grant_status(status: &str) -> HookGrantWhereInput {
    HookGrantWhereInput {
        status: Some(StringFilter {
            eq: Some(status.to_string()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn by_key(key: &str) -> HookRecordWhereInput {
    HookRecordWhereInput {
        record_key: Some(StringFilter {
            eq: Some(key.to_string()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[derive(Clone, Copy, Debug)]
enum Write {
    Insert,
    InsertIfAbsent,
    Upsert,
    UpdateById,
    UpdateByKey,
    UpdateWhere,
    UpdateWhereBounded,
    UpdateIf,
    CompareAndSwap,
    DeleteById,
    DeleteByKey,
    DeleteWhere,
    DeleteWhereBounded,
}

const WRITES: [Write; 13] = [
    Write::Insert,
    Write::InsertIfAbsent,
    Write::Upsert,
    Write::UpdateById,
    Write::UpdateByKey,
    Write::UpdateWhere,
    Write::UpdateWhereBounded,
    Write::UpdateIf,
    Write::CompareAndSwap,
    Write::DeleteById,
    Write::DeleteByKey,
    Write::DeleteWhere,
    Write::DeleteWhereBounded,
];

fn limit() -> MutationLimit {
    MutationLimit::new(10).unwrap()
}

async fn run_write(
    tx: &mut MutationContext<'_>,
    write: Write,
    existing: &HookRecord,
) -> graphql_orm::Result<()> {
    let id = existing.id;
    match write {
        Write::Insert => {
            tx.insert::<HookRecord>(create("new")).await?;
        }
        Write::InsertIfAbsent => {
            tx.insert_if_absent::<HookGrant>(grant("new")).await?;
        }
        Write::Upsert => {
            tx.upsert::<HookRecord>(CreateHookRecordInput {
                record_key: "existing".to_string(),
                status: "after".to_string(),
            })
            .await?;
        }
        Write::UpdateById => {
            tx.update_by_id::<HookRecord>(&id, update()).await?;
        }
        Write::UpdateByKey => {
            tx.update_by_key::<HookGrant>(&grant_key(), grant_update())
                .await?;
        }
        Write::UpdateWhere => {
            tx.update_where::<HookRecord>(by_key("existing"), update())
                .await?;
        }
        Write::UpdateWhereBounded => {
            tx.update_where_bounded::<HookRecord>(by_key("existing"), update(), limit())
                .await?;
        }
        Write::UpdateIf => {
            tx.update_if::<HookGrant>(&grant_key(), grant_status("before"), grant_update())
                .await?;
        }
        Write::CompareAndSwap => {
            tx.compare_and_swap::<HookRecord>(&id, existing.version, by_key("existing"), update())
                .await?;
        }
        Write::DeleteById => {
            tx.delete_by_id::<HookRecord>(&id).await?;
        }
        Write::DeleteByKey => {
            tx.delete_by_key::<HookGrant>(&grant_key()).await?;
        }
        Write::DeleteWhere => {
            tx.delete_where::<HookRecord>(by_key("existing")).await?;
        }
        Write::DeleteWhereBounded => {
            tx.delete_where_bounded::<HookRecord>(by_key("existing"), limit())
                .await?;
        }
    }
    Ok(())
}

async fn assert_unchanged(db: &Database, existing: &HookRecord) {
    let rows = HookRecord::query(db.pool()).fetch_all().await.unwrap();
    assert_eq!(rows.len(), 1, "the refused transaction must roll back");
    assert_eq!(rows[0].id, existing.id);
    assert_eq!(rows[0].status, "before");
    assert_eq!(rows[0].version, existing.version);
    let grants = HookGrant::query(db.pool()).fetch_all().await.unwrap();
    assert_eq!(grants.len(), 1, "the refused transaction must roll back");
    assert_eq!(grants[0].grant_name, "existing");
    assert_eq!(grants[0].status, "before");
}

fn assert_failure(error: &OrmPublicError, failure: Failure, label: &str) {
    match failure {
        Failure::Public(code) | Failure::PublicPropagated(code) => {
            assert_eq!(error.code, code, "{label}: {error:?}");
            assert_eq!(error.message, "refused by hook policy", "{label}");
        }
        _ => {
            assert_eq!(
                error.code,
                OrmErrorCode::InternalError,
                "{label}: {error:?}"
            );
            assert_eq!(error.message, "internal error", "{label}");
            assert!(!format!("{error}").contains("secret"), "{label}");
            assert!(!format!("{error:?}").contains("secret"), "{label}");
        }
    }
    // Rendering at a GraphQL boundary matches the generated mutation path.
    let (code, message) = graphql_rendering(failure);
    let graphql = error.clone().into_graphql_error();
    assert_eq!(graphql.message, message, "{label}");
    assert_eq!(
        graphql.extensions.as_ref().and_then(|ext| ext.get("code")),
        code.map(async_graphql::Value::from).as_ref(),
        "{label}"
    );
}

/// The code and message a GraphQL client sees. An authored refusal without an
/// allow-listed code keeps its existing GraphQL rendering; only repository
/// callers see it sanitized.
fn graphql_rendering(failure: Failure) -> (Option<&'static str>, &'static str) {
    match failure {
        Failure::Public(code) | Failure::PublicPropagated(code) => {
            (Some(code.as_str()), "refused by hook policy")
        }
        Failure::Uncoded => (None, SECRET),
        Failure::UnknownCode => (Some("PRIVATE"), SECRET),
        Failure::Propagated => (Some("INTERNAL_ERROR"), "internal error"),
    }
}

const FAILURES: [Failure; 8] = [
    Failure::Public(OrmErrorCode::Conflict),
    Failure::Public(OrmErrorCode::Forbidden),
    Failure::Public(OrmErrorCode::InvalidInput),
    Failure::Public(OrmErrorCode::NotFound),
    Failure::PublicPropagated(OrmErrorCode::Conflict),
    Failure::Uncoded,
    Failure::UnknownCode,
    Failure::Propagated,
];

async fn transaction_refusals_preserve_only_public_codes() {
    let (db, mut owner, hook) = fixture().await;
    let existing = HookRecord::insert(&db, create("existing")).await.unwrap();
    HookGrant::insert(&db, grant("existing")).await.unwrap();
    for write in WRITES {
        for phase in [MutationPhase::Before, MutationPhase::After] {
            for failure in FAILURES {
                let label = format!("{write:?} {phase:?} {failure:?}");
                *hook.phase.lock().unwrap() = Some((phase.clone(), failure));
                let existing_row = existing.clone();
                let result = db
                    .transaction(TransactionMode::StateMachine, |tx| {
                        Box::pin(async move {
                            run_write(tx, write, &existing_row)
                                .await
                                .map_err(OrmPublicError::from)
                        })
                    })
                    .await;
                *hook.phase.lock().unwrap() = None;
                let error = match result {
                    Err(TransactionError::Rejected(error)) => error,
                    other => panic!("{label}: expected a rejected transaction, got {other:?}"),
                };
                assert_failure(&error, failure, &label);
                assert_unchanged(&db, &existing).await;
            }
        }
    }
    owner.cleanup().unwrap();
}

async fn standalone_refusals_preserve_only_public_codes() {
    let (db, mut owner, hook) = fixture().await;
    let existing = HookRecord::insert(&db, create("existing")).await.unwrap();
    HookGrant::insert(&db, grant("existing")).await.unwrap();
    for phase in [MutationPhase::Before, MutationPhase::After] {
        for failure in FAILURES {
            *hook.phase.lock().unwrap() = Some((phase.clone(), failure));
            let results = [
                ("insert", HookRecord::insert(&db, create("new")).await.err()),
                (
                    "update_by_id",
                    HookRecord::update_by_id(&db, &existing.id, update())
                        .await
                        .err(),
                ),
                (
                    "compare_and_swap",
                    HookRecord::compare_and_swap(
                        &db,
                        &existing.id,
                        existing.version,
                        by_key("existing"),
                        update(),
                    )
                    .await
                    .err(),
                ),
                (
                    "delete_by_id",
                    HookRecord::delete_by_id(&db, &existing.id).await.err(),
                ),
            ];
            *hook.phase.lock().unwrap() = None;
            for (method, error) in results {
                let label = format!("{method} {phase:?} {failure:?}");
                let error = error.unwrap_or_else(|| panic!("{label}: expected a refusal"));
                assert_failure(&OrmPublicError::from(error), failure, &label);
            }
            assert_unchanged(&db, &existing).await;
        }
    }
    owner.cleanup().unwrap();
}

async fn retention_purge_refusals_preserve_only_public_codes() {
    let (db, mut owner, hook) = fixture().await;
    for kind in ["expired", "expired"] {
        HookRetained::insert(
            &db,
            CreateHookRetainedInput {
                kind: kind.to_string(),
            },
        )
        .await
        .unwrap();
    }
    let expired = || HookRetainedWhereInput {
        kind: Some(StringFilter {
            eq: Some("expired".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    for phase in [MutationPhase::Before, MutationPhase::After] {
        for failure in FAILURES {
            let label = format!("purge {phase:?} {failure:?}");
            *hook.phase.lock().unwrap() = Some((phase.clone(), failure));
            let result = db
                .retention_transaction(|maintenance| {
                    Box::pin(async move {
                        maintenance
                            .purge::<HookRetained>(expired(), MutationLimit::new(10)?)
                            .await
                            .map_err(OrmPublicError::from)
                    })
                })
                .await;
            *hook.phase.lock().unwrap() = None;
            let error = match result {
                Err(TransactionError::Rejected(error)) => error,
                other => panic!("{label}: expected a rejected purge, got {other:?}"),
            };
            assert_failure(&error, failure, &label);
            let rows = HookRetained::query(db.pool()).fetch_all().await.unwrap();
            assert_eq!(rows.len(), 2, "{label}: the refused purge must roll back");
        }
    }
    owner.cleanup().unwrap();
}

/// Entity-level write denial on a transaction write keeps its public code.
async fn entity_write_denial_reports_forbidden() {
    let (mut db, mut owner, _hook) = fixture().await;
    let existing = HookRecord::insert(&db, create("existing")).await.unwrap();
    db.set_entity_policy(DenyWrites);
    let id = existing.id;
    let result = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                tx.update_by_id::<HookRecord>(&id, update())
                    .await
                    .map_err(OrmPublicError::from)
            })
        })
        .await;
    let error = match result {
        Err(TransactionError::Rejected(error)) => error,
        other => panic!("expected a rejected transaction, got {other:?}"),
    };
    assert_eq!(error.code, OrmErrorCode::Forbidden, "{error:?}");
    owner.cleanup().unwrap();
}

/// Generated GraphQL mutations return the same public code and message.
async fn graphql_refusals_match_repository_refusals() {
    let (db, mut owner, hook) = fixture().await;
    let existing = HookRecord::insert(&db, create("existing")).await.unwrap();
    HookGrant::insert(&db, grant("existing")).await.unwrap();
    let schema = schema_builder(db.clone())
        .data(AuthSubject::new("test-principal"))
        .finish();
    let id = existing.id;
    let mutations = [
        "createHookRecord(input: {recordKey: \"new\", status: \"before\"}) { success }".to_string(),
        format!("updateHookRecord(id: \"{id}\", input: {{status: \"after\"}}) {{ success }}"),
        format!("deleteHookRecord(id: \"{id}\") {{ success }}"),
    ];
    for phase in [MutationPhase::Before, MutationPhase::After] {
        for failure in FAILURES {
            for mutation in &mutations {
                let label = format!("{mutation} {phase:?} {failure:?}");
                *hook.phase.lock().unwrap() = Some((phase.clone(), failure));
                let response = schema.execute(format!("mutation {{ {mutation} }}")).await;
                *hook.phase.lock().unwrap() = None;
                assert_eq!(response.errors.len(), 1, "{label}: {response:?}");
                let (code, message) = graphql_rendering(failure);
                assert_eq!(response.errors[0].message, message, "{label}");
                assert_eq!(
                    response.errors[0]
                        .extensions
                        .as_ref()
                        .and_then(|ext| ext.get("code")),
                    code.map(async_graphql::Value::from).as_ref(),
                    "{label}"
                );
                assert_unchanged(&db, &existing).await;
            }
        }
    }
    owner.cleanup().unwrap();
}

macro_rules! case {
    ($name:ident) => {
        #[tokio::test]
        #[cfg_attr(
            feature = "postgres",
            ignore = "requires test-owned disposable PostgreSQL"
        )]
        async fn $name() {
            super::$name().await;
        }
    };
}

mod cases {
    case!(transaction_refusals_preserve_only_public_codes);
    case!(standalone_refusals_preserve_only_public_codes);
    case!(retention_purge_refusals_preserve_only_public_codes);
    case!(entity_write_denial_reports_forbidden);
    case!(graphql_refusals_match_repository_refusals);
}
