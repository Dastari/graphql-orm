//! Repository host timestamps; PostgreSQL execution uses only test-owned containers.
#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
use graphql_orm::{async_graphql, prelude::*};
use std::sync::{Arc, Mutex};

#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[repository_entity(table = "clocked_records", plural = "ClockedRecords", upsert = "label")]
struct ClockedRecord {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: graphql_orm::uuid::Uuid,
    #[unique]
    #[filterable(type = "string")]
    label: String,
    #[graphql_orm(timestamp = "host", default = "0", write_policy = "clock.write")]
    #[filterable(type = "number")]
    created_at: i64,
    #[graphql_orm(
        timestamp = "host",
        db_column = "updated_at",
        default = "0",
        write_policy = "clock.write"
    )]
    #[filterable(type = "number")]
    modified_ms: i64,
    #[graphql_orm(version, default = "0")]
    revision: i64,
}

// Identical physical declaration without the opt-in, including explicit defaults.
#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    table = "clocked_records",
    plural = "LegacyClockedRecords",
    upsert = "label"
)]
struct LegacyClockedRecord {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: graphql_orm::uuid::Uuid,
    #[unique]
    #[filterable(type = "string")]
    label: String,
    #[graphql_orm(default = "0", write_policy = "clock.write")]
    #[filterable(type = "number")]
    created_at: i64,
    #[graphql_orm(db_column = "updated_at", default = "0", write_policy = "clock.write")]
    #[filterable(type = "number")]
    modified_ms: i64,
    #[graphql_orm(version, default = "0")]
    revision: i64,
}

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    table = "nullable_clocks",
    plural = "NullableClocks",
    repository_mutations = true,
    unique_composite = "partition,name",
    upsert = "partition,name",
    write_policy = "clock.write"
)]
struct NullableClock {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    partition: String,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    name: String,
    #[graphql_orm(timestamp = "host", default = false, write_policy = "clock.write")]
    #[filterable(type = "number")]
    created_at: Option<i64>,
    #[graphql_orm(timestamp = "host", db_column = "updated_at", default = false)]
    modified_ms: Option<i64>,
}

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(table = "mixed_clocks", plural = "MixedClocks", upsert = "label")]
struct MixedClock {
    #[primary_key]
    id: String,
    #[unique]
    label: String,
    #[graphql_orm(timestamp = "host", default = "0")]
    created_at: i64,
    #[graphql_orm(default = "0")]
    updated_at: i64,
}

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(table = "implicit_clocks", plural = "ImplicitClocks")]
struct ImplicitClock {
    #[primary_key]
    id: String,
    #[graphql_orm(timestamp = "host")]
    created_at: i64,
    #[graphql_orm(timestamp = "host")]
    updated_at: i64,
}
#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(table = "implicit_clocks", plural = "LegacyImplicitClocks")]
struct LegacyImplicitClock {
    #[primary_key]
    id: String,
    created_at: i64,
    updated_at: i64,
}

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(table = "clock_journal", plural = "ClockJournals")]
struct ClockJournal {
    #[primary_key]
    id: String,
    #[unique]
    marker: String,
}

#[derive(Clone)]
struct AllowEntities;
impl EntityPolicy<Backend> for AllowEntities {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<Backend>,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}
#[derive(Clone)]
struct Fields(bool);
impl FieldPolicy<Backend> for Fields {
    fn can_read_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database<Backend>,
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
        _: &'a Database<Backend>,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
    fn can_write_repository_field<'a>(
        &'a self,
        _: Option<AccessContext<'a>>,
        _: &'a Database<Backend>,
        _: &'static str,
        _: &'static str,
        policy: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(self.0 || policy.is_none()) })
    }
}

struct TestDatabase {
    db: Database<Backend>,
    #[cfg(feature = "postgres")]
    owned: owned_postgres::OwnedPostgres,
}
impl TestDatabase {
    async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        #[cfg(feature = "sqlite")]
        let db = Database::<Backend>::connect_sqlite_with_options(
            "sqlite::memory:",
            graphql_orm::db::ConnectionOptions::default().max_connections(1),
        )
        .await?;
        #[cfg(feature = "postgres")]
        let owned = owned_postgres::OwnedPostgres::start("host-timestamps")?;
        #[cfg(feature = "postgres")]
        let db = Database::<Backend>::connect_postgres_with_options(
            &owned.url,
            graphql_orm::db::ConnectionOptions::default().max_connections(1),
        )
        .await?;
        let mut fixture = Self {
            db,
            #[cfg(feature = "postgres")]
            owned,
        };
        fixture.db.set_entity_policy(AllowEntities);
        fixture.db.set_field_policy(Fields(true));
        let plan = fixture
            .db
            .schema()
            .plan_migration_to_entities(
                "clock-init",
                "owned timestamp fixture",
                &[
                    ClockedRecord::metadata(),
                    NullableClock::metadata(),
                    MixedClock::metadata(),
                    ImplicitClock::metadata(),
                    ClockJournal::metadata(),
                ],
            )
            .await?;
        fixture
            .db
            .schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await?;
        Ok(fixture)
    }
    #[cfg(feature = "postgres")]
    fn cleanup(mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.owned.cleanup()
    }
    #[cfg(feature = "sqlite")]
    fn cleanup(self) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }
}
fn input(label: &str, value: i64) -> CreateClockedRecordInput {
    CreateClockedRecordInput {
        id: graphql_orm::uuid::Uuid::new_v4(),
        label: label.into(),
        created_at: value,
        modified_ms: value,
    }
}
fn filter(label: &str) -> ClockedRecordWhereInput {
    ClockedRecordWhereInput {
        label: Some(StringFilter {
            eq: Some(label.into()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "owns a disposable Docker PostgreSQL container"
)]
async fn exact_values_direct_and_transaction_updates_cas_and_bounded_sets()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = TestDatabase::new().await?;
    let db = &fixture.db;
    for value in [1_765_432_100_123, 0, -123, 42, i64::MIN, i64::MAX] {
        let supplied = input("exact", value);
        let id = supplied.id;
        let row = ClockedRecord::insert(db, supplied).await?;
        assert_eq!(row.id, id);
        assert_eq!((row.created_at, row.modified_ms), (value, value));
        let read = ClockedRecord::find_by_id(db, &id).await?.unwrap();
        assert_eq!((read.created_at, read.modified_ms), (value, value));
        let row = ClockedRecord::update_by_id(
            db,
            &id,
            UpdateClockedRecordInput {
                label: Some("changed".into()),
                ..Default::default()
            },
        )
        .await?
        .unwrap();
        assert_eq!((row.created_at, row.modified_ms), (value, value));
        let row = ClockedRecord::update_by_id(
            db,
            &id,
            UpdateClockedRecordInput {
                modified_ms: Some(value.wrapping_add(1)),
                ..Default::default()
            },
        )
        .await?
        .unwrap();
        assert_eq!(row.modified_ms, value.wrapping_add(1));
        ClockedRecord::delete_by_id(db, &id).await?;
    }
    let row = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                let row = tx
                    .insert::<ClockedRecord>(input("transaction", -77))
                    .await?;
                let row = tx
                    .update_by_id::<ClockedRecord>(
                        &row.id,
                        UpdateClockedRecordInput {
                            modified_ms: Some(1_765_432_100_999),
                            ..Default::default()
                        },
                    )
                    .await?
                    .unwrap();
                let success = tx
                    .compare_and_swap::<ClockedRecord>(
                        &row.id,
                        row.revision,
                        ClockedRecordWhereInput::default(),
                        UpdateClockedRecordInput {
                            modified_ms: Some(-999),
                            ..Default::default()
                        },
                    )
                    .await?;
                assert!(matches!(success, ConditionalUpdateOutcome::Updated(_)));
                let conflict = tx
                    .compare_and_swap::<ClockedRecord>(
                        &row.id,
                        row.revision,
                        ClockedRecordWhereInput::default(),
                        UpdateClockedRecordInput {
                            modified_ms: Some(10),
                            ..Default::default()
                        },
                    )
                    .await?;
                assert!(matches!(conflict, ConditionalUpdateOutcome::Conflict));
                tx.find_by_id::<ClockedRecord>(&row.id)
                    .await
                    .map_err(OrmPublicError::from)
            })
        })
        .await?
        .unwrap();
    assert_eq!((row.created_at, row.modified_ms), (-77, -999));
    let success = ClockedRecord::compare_and_swap(
        db,
        &row.id,
        row.revision,
        ClockedRecordWhereInput::default(),
        UpdateClockedRecordInput {
            modified_ms: Some(-444),
            ..Default::default()
        },
    )
    .await?;
    assert!(matches!(success, ConditionalUpdateOutcome::Updated(_)));
    assert!(matches!(
        ClockedRecord::compare_and_swap(
            db,
            &row.id,
            row.revision,
            ClockedRecordWhereInput::default(),
            UpdateClockedRecordInput {
                modified_ms: Some(55),
                ..Default::default()
            }
        )
        .await?,
        ConditionalUpdateOutcome::Conflict
    ));
    ClockedRecord::update_where(
        db,
        filter("transaction"),
        UpdateClockedRecordInput {
            modified_ms: Some(-333),
            ..Default::default()
        },
    )
    .await?;

    let result = ClockedRecord::update_where_bounded(
        db,
        filter("transaction"),
        UpdateClockedRecordInput {
            created_at: Some(123_456_789_123),
            ..Default::default()
        },
        MutationLimit::new(1)?,
    )
    .await?;
    assert!(matches!(result, BoundedMutationOutcome::Applied { .. }));
    db.transaction(TransactionMode::StateMachine, |tx| {
        Box::pin(async move {
            tx.update_where::<ClockedRecord>(
                filter("transaction"),
                UpdateClockedRecordInput {
                    modified_ms: Some(-876),
                    ..Default::default()
                },
            )
            .await?;
            tx.update_where_bounded::<ClockedRecord>(
                filter("transaction"),
                UpdateClockedRecordInput {
                    modified_ms: Some(-987),
                    ..Default::default()
                },
                MutationLimit::new(1)?,
            )
            .await?;
            Ok::<_, OrmPublicError>(())
        })
    })
    .await?;
    assert_eq!(
        ClockedRecord::find_by_id(db, &row.id)
            .await?
            .unwrap()
            .modified_ms,
        -987
    );
    fixture.cleanup()
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "owns a disposable Docker PostgreSQL container"
)]
async fn nullable_complete_key_conditional_insert_if_absent_and_upsert()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = TestDatabase::new().await?;
    let db = &fixture.db;
    let make = || CreateNullableClockInput {
        partition: "owned".into(),
        name: "first".into(),
        created_at: Some(-17),
        modified_ms: None,
    };
    let key = NullableClockKey {
        partition: "owned".into(),
        name: "first".into(),
    };
    assert!(matches!(
        NullableClock::insert_if_absent(db, make()).await?,
        InsertIfAbsentOutcome::Inserted(_)
    ));
    assert!(matches!(
        NullableClock::insert_if_absent(db, make()).await?,
        InsertIfAbsentOutcome::AlreadyPresent(_)
    ));
    let row = NullableClock::update_by_key(
        db,
        &key,
        UpdateNullableClockInput {
            modified_ms: Some(Some(1_765_432_100_123)),
            ..Default::default()
        },
    )
    .await?
    .unwrap();
    assert_eq!(
        (row.created_at, row.modified_ms),
        (Some(-17), Some(1_765_432_100_123))
    );
    let expected = || NullableClockWhereInput {
        created_at: Some(IntFilter {
            eq: Some(-17),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(matches!(
        NullableClock::update_if(
            db,
            &key,
            expected(),
            UpdateNullableClockInput {
                created_at: Some(None),
                ..Default::default()
            }
        )
        .await?,
        PredicateUpdateOutcome::Updated(_)
    ));
    assert!(matches!(
        NullableClock::update_if(
            db,
            &key,
            expected(),
            UpdateNullableClockInput {
                modified_ms: Some(Some(5)),
                ..Default::default()
            }
        )
        .await?,
        PredicateUpdateOutcome::PredicateConflict
    ));
    let row = NullableClock::find_by_key(db, &key).await?.unwrap();
    assert_eq!(
        (row.created_at, row.modified_ms),
        (None, Some(1_765_432_100_123))
    );
    db.transaction(TransactionMode::StateMachine, |tx| {
        Box::pin(async move {
            tx.update_by_key::<NullableClock>(
                &key,
                UpdateNullableClockInput {
                    modified_ms: Some(None),
                    ..Default::default()
                },
            )
            .await?;
            tx.update_if::<NullableClock>(
                &key,
                NullableClockWhereInput {
                    created_at: Some(IntFilter {
                        is_null: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                UpdateNullableClockInput {
                    created_at: Some(Some(42)),
                    ..Default::default()
                },
            )
            .await?;
            let outcome = tx
                .upsert::<NullableClock>(CreateNullableClockInput {
                    partition: key.partition.clone(),
                    name: key.name.clone(),
                    created_at: Some(-99),
                    modified_ms: Some(0),
                })
                .await?;
            assert_eq!(
                (outcome.entity.created_at, outcome.entity.modified_ms),
                (Some(-99), Some(0))
            );
            let absent = tx
                .insert_if_absent::<NullableClock>(CreateNullableClockInput {
                    partition: "owned".into(),
                    name: "second".into(),
                    created_at: None,
                    modified_ms: Some(i64::MAX),
                })
                .await?;
            assert!(matches!(absent, InsertIfAbsentOutcome::Inserted(_)));
            Ok::<_, OrmPublicError>(())
        })
    })
    .await?;
    let supplied = input("upsert", 123_456_789_999);
    let id = supplied.id;
    assert_eq!(ClockedRecord::upsert(db, supplied).await?.entity.id, id);
    let changed = ClockedRecord::upsert(db, input("upsert", -1)).await?;
    assert_eq!(
        (changed.entity.created_at, changed.entity.modified_ms),
        (-1, -1)
    );
    assert_eq!(changed.entity.id, id);
    fixture.cleanup()
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "owns a disposable Docker PostgreSQL container"
)]
async fn mixed_legacy_timestamps_and_annotation_only_migration_noop()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = TestDatabase::new().await?;
    let db = &fixture.db;
    for (host, legacy) in [
        (ClockedRecord::metadata(), LegacyClockedRecord::metadata()),
        (ImplicitClock::metadata(), LegacyImplicitClock::metadata()),
    ] {
        let metadata_unchanged = format!("{:?}", host.fields) == format!("{:?}", legacy.fields);
        assert!(metadata_unchanged, "annotation changed field metadata");
        let mut all = vec![
            ClockedRecord::metadata(),
            NullableClock::metadata(),
            MixedClock::metadata(),
            ImplicitClock::metadata(),
            ClockJournal::metadata(),
        ];
        *all.iter_mut()
            .find(|entity| entity.table_name == legacy.table_name)
            .unwrap() = legacy;
        let plan = db
            .schema()
            .plan_migration_to_entities("legacy-target", "unchanged physical target", &all)
            .await?;
        let differences = plan
            .steps
            .iter()
            .map(|step| match &step.step {
                MigrationStep::AlterColumn {
                    table_name, after, ..
                } => ("alter_column", table_name.as_str(), after.name.as_str()),
                MigrationStep::CreateIndex { table_name, index } => {
                    ("create_index", table_name.as_str(), index.name)
                }
                MigrationStep::DropIndex {
                    table_name,
                    index_name,
                } => ("drop_index", table_name.as_str(), index_name.as_str()),
                _ => ("other", "", ""),
            })
            .collect::<Vec<_>>();
        assert!(
            differences.is_empty(),
            "physical metadata differences: {differences:?}"
        );
        *all.iter_mut()
            .find(|entity| entity.table_name == host.table_name)
            .unwrap() = host;
        let plan = db
            .schema()
            .plan_migration_to_entities("host-target", "annotation only", &all)
            .await?;
        let differences = plan
            .steps
            .iter()
            .map(|step| match &step.step {
                MigrationStep::AlterColumn {
                    table_name, after, ..
                } => ("alter_column", table_name.as_str(), after.name.as_str()),
                MigrationStep::CreateIndex { table_name, index } => {
                    ("create_index", table_name.as_str(), index.name)
                }
                MigrationStep::DropIndex {
                    table_name,
                    index_name,
                } => ("drop_index", table_name.as_str(), index_name.as_str()),
                _ => ("other", "", ""),
            })
            .collect::<Vec<_>>();
        assert!(
            differences.is_empty(),
            "physical metadata differences: {differences:?}"
        );
    }
    let row = MixedClock::insert(
        db,
        CreateMixedClockInput {
            label: "mixed".into(),
            created_at: -123,
        },
    )
    .await?;
    assert_eq!((row.created_at, row.updated_at), (-123, 0));
    let row = MixedClock::update_by_id(
        db,
        &row.id,
        UpdateMixedClockInput {
            created_at: Some(1_765_432_100_123),
            ..Default::default()
        },
    )
    .await?
    .unwrap();
    assert_eq!(row.created_at, 1_765_432_100_123);
    assert!(row.updated_at > 1_000_000_000 && row.updated_at < 10_000_000_000);
    let row = MixedClock::upsert(
        db,
        CreateMixedClockInput {
            label: "mixed".into(),
            created_at: -42,
        },
    )
    .await?
    .entity;
    assert_eq!(row.created_at, -42);
    assert!(row.updated_at > 1_000_000_000 && row.updated_at < 10_000_000_000);
    let runtime = RuntimeSchema::from_static_entities(&[MixedClock::metadata()])?;
    assert!(
        runtime
            .collections
            .iter()
            .flat_map(|c| &c.fields)
            .filter(|f| matches!(f.physical_column.as_str(), "created_at" | "updated_at"))
            .all(|f| f.value_kind == RuntimeValueKind::Integer
                && f.default == Some(RuntimeDefault::Literal("0".into())))
    );
    fixture.cleanup()
}

#[derive(Clone, Default)]
struct RecordingHook(Arc<Mutex<Vec<MutationEvent>>>);
impl MutationHook<Backend> for RecordingHook {
    fn on_mutation<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a mut MutationContext<'_, Backend>,
        event: &'a MutationEvent,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            self.0.lock().unwrap().push(event.clone());
            Ok(())
        })
    }
}
#[derive(Clone)]
struct FailingJournal;
impl MutationHook<Backend> for FailingJournal {
    fn on_mutation<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        tx: &'a mut MutationContext<'_, Backend>,
        event: &'a MutationEvent,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if event.entity_name == "ClockedRecord" && event.phase == MutationPhase::After {
                tx.insert::<ClockJournal>(CreateClockJournalInput {
                    marker: "same-marker".into(),
                })
                .await
                .map_err(|_| async_graphql::Error::new("journal failed"))?;
                return Err(async_graphql::Error::new("hook failed"));
            }
            Ok(())
        })
    }
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "owns a disposable Docker PostgreSQL container"
)]
async fn denied_timestamp_writes_precede_dml_and_hook_journal_errors_rollback()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = TestDatabase::new().await?;
    let hook = RecordingHook::default();
    fixture.db.set_mutation_hook(hook.clone());
    let row = ClockedRecord::insert(&fixture.db, input("protected", -12)).await?;
    hook.0.lock().unwrap().clear();
    fixture.db.set_field_policy(Fields(false));
    assert!(
        ClockedRecord::insert(&fixture.db, input("denied", -3))
            .await
            .is_err()
    );
    assert!(
        ClockedRecord::update_by_id(
            &fixture.db,
            &row.id,
            UpdateClockedRecordInput {
                modified_ms: Some(99),
                ..Default::default()
            }
        )
        .await
        .is_err()
    );
    assert!(
        ClockedRecord::upsert(&fixture.db, input("protected", 99))
            .await
            .is_err()
    );
    assert!(hook.0.lock().unwrap().is_empty());
    let read = ClockedRecord::find_by_id(&fixture.db, &row.id)
        .await?
        .unwrap();
    assert_eq!((read.created_at, read.modified_ms), (-12, -12));
    fixture.db.set_field_policy(Fields(true));
    ClockedRecord::update_by_id(
        &fixture.db,
        &row.id,
        UpdateClockedRecordInput {
            modified_ms: Some(-77),
            ..Default::default()
        },
    )
    .await?;
    assert!(hook.0.lock().unwrap().iter().any(|event| {
        event.phase == MutationPhase::After
            && event
                .changes
                .iter()
                .any(|change| change.field == "updated_at" && change.value == SqlValue::Int(-77))
    }));
    fixture.db.set_mutation_hook(FailingJournal);
    let failed = fixture
        .db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                tx.update_by_id::<ClockedRecord>(
                    &row.id,
                    UpdateClockedRecordInput {
                        modified_ms: Some(1_765_432_100_123),
                        ..Default::default()
                    },
                )
                .await?;
                Ok::<_, OrmPublicError>(())
            })
        })
        .await;
    assert!(failed.is_err());
    assert_eq!(
        ClockedRecord::find_by_id(&fixture.db, &row.id)
            .await?
            .unwrap()
            .modified_ms,
        -77
    );
    assert_eq!(ClockJournal::count_all(&fixture.db).await?, 0);
    assert!(
        ClockedRecord::insert(&fixture.db, input("hook-denied", 88))
            .await
            .is_err()
    );
    assert_eq!(
        ClockedRecord::query(&fixture.db)
            .filter(filter("hook-denied"))
            .fetch_all()
            .await?
            .len(),
        0
    );
    ClockJournal::insert(
        &fixture.db,
        CreateClockJournalInput {
            marker: "same-marker".into(),
        },
    )
    .await?;
    assert!(
        fixture
            .db
            .transaction(TransactionMode::StateMachine, |tx| Box::pin(async move {
                tx.insert::<ClockedRecord>(input("journal-denied", 88))
                    .await?;
                Ok::<_, OrmPublicError>(())
            }))
            .await
            .is_err()
    );
    assert_eq!(
        ClockedRecord::query(&fixture.db)
            .filter(filter("journal-denied"))
            .fetch_all()
            .await?
            .len(),
        0
    );
    assert_eq!(ClockJournal::count_all(&fixture.db).await?, 1);
    fixture.cleanup()
}

#[derive(Clone)]
struct PendingHook {
    entered: Arc<tokio::sync::Notify>,
}
impl MutationHook<Backend> for PendingHook {
    fn on_mutation<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a mut MutationContext<'_, Backend>,
        event: &'a MutationEvent,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if event.entity_name == "ClockedRecord" && event.phase == MutationPhase::After {
                self.entered.notify_one();
                std::future::pending::<()>().await;
            }
            Ok(())
        })
    }
}
#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "owns a disposable Docker PostgreSQL container"
)]
async fn whole_transaction_cancellation_with_staged_journal_or_pending_hook_rolls_back()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = TestDatabase::new().await?;
    for pending_hook in [false, true] {
        let entered = Arc::new(tokio::sync::Notify::new());
        if pending_hook {
            fixture.db.set_mutation_hook(PendingHook {
                entered: entered.clone(),
            });
        }
        let mut events = fixture
            .db
            .ensure_event_sender::<ClockedRecordChangedEvent>()
            .subscribe();
        let task = {
            let db = fixture.db.clone();
            let entered = entered.clone();
            tokio::spawn(async move {
                db.transaction(TransactionMode::StateMachine, |tx| {
                    Box::pin(async move {
                        tx.insert::<ClockJournal>(CreateClockJournalInput {
                            marker: "cancelled".into(),
                        })
                        .await?;
                        tx.insert::<ClockedRecord>(input("cancelled", -123)).await?;
                        entered.notify_one();
                        std::future::pending::<()>().await;
                        Ok::<_, OrmPublicError>(())
                    })
                })
                .await
            })
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), entered.notified()).await?;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(events.try_recv().is_err());
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            assert_eq!(ClockedRecord::count_all(&fixture.db).await?, 0);
            assert_eq!(ClockJournal::count_all(&fixture.db).await?, 0);
            Ok::<_, graphql_orm::sqlx::Error>(())
        })
        .await??;
    }
    fixture.cleanup()
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "legacy_public_clocks", plural = "LegacyPublicClocks")]
struct LegacyPublicClock {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    label: String,
    #[graphql_orm(default = "0")]
    created_at: i64,
    #[graphql_orm(default = "0")]
    updated_at: i64,
}
schema_roots! { query_custom_ops: [], entities: [LegacyPublicClock], }

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "owns a disposable Docker PostgreSQL container"
)]
async fn legacy_graphql_sdl_and_automatic_repository_writes_are_unchanged()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = TestDatabase::new().await?;
    let plan = fixture
        .db
        .schema()
        .plan_migration_to_entities(
            "legacy-public",
            "owned legacy GraphQL fixture",
            &[
                ClockedRecord::metadata(),
                NullableClock::metadata(),
                MixedClock::metadata(),
                ImplicitClock::metadata(),
                ClockJournal::metadata(),
                LegacyPublicClock::metadata(),
            ],
        )
        .await?;
    fixture
        .db
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    let schema = async_graphql::Schema::build(
        QueryRoot::default(),
        MutationRoot::default(),
        SubscriptionRoot::default(),
    )
    .finish();
    let sdl = schema.sdl();
    let create = sdl
        .split("input CreateLegacyPublicClockInput {")
        .nth(1)
        .expect("legacy create input")
        .split('}')
        .next()
        .unwrap();
    let update = sdl
        .split("input UpdateLegacyPublicClockInput {")
        .nth(1)
        .expect("legacy update input")
        .split('}')
        .next()
        .unwrap();
    assert_eq!(create.trim(), "label: String!");
    assert_eq!(update.trim(), "label: String");
    let object = sdl
        .split("type LegacyPublicClock {")
        .nth(1)
        .expect("legacy object")
        .split('}')
        .next()
        .unwrap();
    assert!(object.contains("createdAt: Int!") && object.contains("updatedAt: Int!"));
    let row = LegacyPublicClock::insert(
        &fixture.db,
        CreateLegacyPublicClockInput {
            label: "legacy".into(),
        },
    )
    .await?;
    assert_eq!((row.created_at, row.updated_at), (0, 0));
    let row = LegacyPublicClock::update_by_id(
        &fixture.db,
        &row.id,
        UpdateLegacyPublicClockInput {
            label: Some("updated".into()),
        },
    )
    .await?
    .unwrap();
    assert_eq!(row.created_at, 0);
    assert!(row.updated_at > 1_000_000_000 && row.updated_at < 10_000_000_000);
    fixture.cleanup()
}
