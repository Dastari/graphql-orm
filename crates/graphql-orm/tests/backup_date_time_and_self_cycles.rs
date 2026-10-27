//! Backup and restore of `DateTime` columns and of self-referencing rows that form cycles, on the
//! enabled backend. Each PostgreSQL case owns its servers.
#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
use graphql_orm::{graphql::orm::*, prelude::*};
use std::collections::BTreeMap;
#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

#[cfg(feature = "sqlite")]
const DATE_TIME: &str = "TEXT";
#[cfg(feature = "postgres")]
const DATE_TIME: &str = "TIMESTAMPTZ";

struct Db {
    db: Database<Backend>,
    #[cfg(feature = "sqlite")]
    path: std::path::PathBuf,
    #[cfg(feature = "postgres")]
    owned: owned_postgres::OwnedPostgres,
}
#[cfg(feature = "sqlite")]
impl Drop for Db {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
async fn open(label: &str, ddl: &[&str]) -> Db {
    #[cfg(feature = "sqlite")]
    let path = std::env::temp_dir().join(format!(
        "graphql-orm-{label}-{}.sqlite3",
        graphql_orm::uuid::Uuid::new_v4().simple()
    ));
    #[cfg(feature = "sqlite")]
    let db = Database::<Backend>::connect_sqlite(format!("sqlite://{}?mode=rwc", path.display()))
        .await
        .unwrap();
    #[cfg(feature = "postgres")]
    let owned = owned_postgres::OwnedPostgres::start(label).unwrap();
    #[cfg(feature = "postgres")]
    let db = Database::<Backend>::connect_postgres(&owned.url)
        .await
        .unwrap();
    for sql in ddl {
        exec(&db, sql).await.unwrap();
    }
    Db {
        db,
        #[cfg(feature = "sqlite")]
        path,
        #[cfg(feature = "postgres")]
        owned,
    }
}
async fn exec(db: &Database<Backend>, sql: &str) -> graphql_orm::Result<()> {
    Backend::fetch_rows(db.pool(), sql, &[]).await.map(|_| ())
}
async fn count(db: &Database<Backend>, table: &str) -> i64 {
    use graphql_orm::sqlx::Row;
    Backend::fetch_rows(db.pool(), &format!("SELECT COUNT(*) FROM {table}"), &[])
        .await
        .unwrap()[0]
        .try_get::<i64, _>(0)
        .unwrap()
}

fn column(name: &str, kind: BackupValueKind, nullable: bool) -> ColumnBackupDescriptor {
    ColumnBackupDescriptor {
        column_name: name.into(),
        rust_field_name: name.into(),
        logical_type: kind,
        decimal: None,
        nullable,
        is_primary_key: name == "id",
        is_generated: false,
        backup_policy: ColumnBackupPolicy::Include,
    }
}
/// A single-key table; `parent` names a self-reference column onto `id`.
fn table(
    name: &str,
    columns: Vec<ColumnBackupDescriptor>,
    parent: Option<&str>,
) -> EntityBackupDescriptor {
    let dependencies = parent
        .map(|source| {
            let nullable = columns
                .iter()
                .find(|c| c.column_name == source)
                .unwrap()
                .nullable;
            EntityDependencyDescriptor {
                entity_name: name.into(),
                table_name: name.into(),
                source_column: source.into(),
                target_column: "id".into(),
                nullable,
                on_delete: DeletePolicy::Restrict,
            }
        })
        .into_iter()
        .collect();
    EntityBackupDescriptor {
        entity_name: name.into(),
        table_name: name.into(),
        primary_key_column: "id".into(),
        primary_key_columns: Vec::new(),
        retention_purge: false,
        export_order: 10,
        restore_order: 10,
        columns,
        dependencies,
    }
}
fn snapshot(entities: &[EntityBackupDescriptor]) -> GraphqlOrmSchemaSnapshot {
    GraphqlOrmSchemaSnapshot {
        backend: format!("{:?}", Backend::DIALECT),
        migration_version: "date-time-self-cycles".into(),
        schema_hash: stable_schema_hash(entities),
        entities: entities.to_vec(),
    }
}
async fn export(
    db: &Database<Backend>,
    entities: &[EntityBackupDescriptor],
) -> graphql_orm::Result<BTreeMap<String, Vec<BackupRow>>> {
    let mut snap = db.begin_consistent_snapshot().await?;
    let mut rows = BTreeMap::new();
    for entity in entities {
        rows.insert(
            entity.table_name.clone(),
            db.export_table_rows(&mut snap, entity).await?,
        );
    }
    Ok(rows)
}
async fn restore(
    db: &Database<Backend>,
    entities: &[EntityBackupDescriptor],
    rows: &BTreeMap<String, Vec<BackupRow>>,
) -> graphql_orm::Result<Vec<ImportReport>> {
    let snapshot = snapshot(entities);
    db.restore_backup_rows(
        &snapshot,
        &snapshot,
        rows,
        &RestoreContext::empty_database(),
    )
    .await
}
fn value<'a>(
    rows: &'a BTreeMap<String, Vec<BackupRow>>,
    table: &str,
    id: &str,
    column: &str,
) -> &'a BackupValue {
    &rows[table]
        .iter()
        .find(|row| row.primary_key == id)
        .unwrap()
        .values[column]
}

fn events_ddl() -> String {
    format!(
        "CREATE TABLE dt_events (id BIGINT PRIMARY KEY, created_at {DATE_TIME} NOT NULL, updated_at {DATE_TIME} NULL, label TEXT NOT NULL)"
    )
}
fn events() -> Vec<EntityBackupDescriptor> {
    vec![table(
        "dt_events",
        vec![
            column("id", BackupValueKind::Integer, false),
            column("created_at", BackupValueKind::DateTime, false),
            column("updated_at", BackupValueKind::DateTime, true),
            column("label", BackupValueKind::String, false),
        ],
        None,
    )]
}

#[tokio::test]
async fn date_times_round_trip_exactly_and_independently_of_the_session_time_zone() {
    let ddl = events_ddl();
    let source = open("dt-source", &[&ddl]).await;
    // Past instants with non-zero microseconds, which no clock re-stamp could reproduce; a NULL;
    // and an equal instant written with a non-UTC offset.
    exec(
        &source.db,
        "INSERT INTO dt_events (id, created_at, updated_at, label) VALUES \
         (1, '2001-02-03T04:05:06.123456Z', '1999-12-31T23:59:59.999999Z', 'a'), \
         (2, '2026-10-27T00:00:00.000001Z', NULL, 'b'), \
         (3, '2001-02-03T14:05:06.123456+10:00', '2001-02-03T04:05:06.123456Z', 'c')",
    )
    .await
    .unwrap();
    let entities = events();
    let rows = export(&source.db, &entities).await.unwrap();
    let text = |value: &str| BackupValue::String(value.into());
    assert_eq!(
        value(&rows, "dt_events", "1", "created_at"),
        &text("2001-02-03T04:05:06.123456Z")
    );
    assert_eq!(
        value(&rows, "dt_events", "1", "updated_at"),
        &text("1999-12-31T23:59:59.999999Z")
    );
    assert_eq!(
        value(&rows, "dt_events", "2", "updated_at"),
        &BackupValue::Null
    );
    assert_eq!(
        value(&rows, "dt_events", "3", "created_at"),
        &text("2001-02-03T04:05:06.123456Z")
    );

    #[cfg(feature = "sqlite")]
    let target = open("dt-target", &[&ddl]).await;
    // The target's sessions render timestamps at +05:45, so any text path would shift values.
    #[cfg(feature = "postgres")]
    let target = {
        let mut target = open("dt-target", &[&ddl]).await;
        target.db.pool().close().await;
        // Set every physical connection's session zone explicitly. A database default can be
        // overridden by the driver's startup settings and would leave this test exercising UTC.
        let pool = graphql_orm::sqlx::postgres::PgPoolOptions::new()
            .after_connect(|connection, _| {
                Box::pin(async move {
                    graphql_orm::sqlx::query("SET TIME ZONE 'Asia/Kathmandu'")
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&target.owned.url)
            .await
            .unwrap();
        target.db = Database::<Backend>::new(pool);
        exec(
            &target.db,
            "CREATE TABLE tz_probe AS SELECT TIMESTAMPTZ '2001-02-03T04:05:06Z'::text AS rendered",
        )
        .await
        .unwrap();
        use graphql_orm::sqlx::Row;
        let rendered: String =
            Backend::fetch_rows(target.db.pool(), "SELECT rendered FROM tz_probe", &[])
                .await
                .unwrap()[0]
                .try_get(0)
                .unwrap();
        assert!(rendered.ends_with("+05:45"), "{rendered}");
        exec(&target.db, "DROP TABLE tz_probe").await.unwrap();
        target
    };

    let reports = restore(&target.db, &entities, &rows).await.unwrap();
    assert_eq!(reports[0].rows_imported, 3);
    // Stored values, never re-stamped: the restored export is identical, row hashes included.
    assert_eq!(export(&target.db, &entities).await.unwrap(), rows);
    // SQLite stores the canonical text it restored.
    #[cfg(feature = "sqlite")]
    {
        use graphql_orm::sqlx::Row;
        let stored: String = Backend::fetch_rows(
            target.db.pool(),
            "SELECT created_at FROM dt_events WHERE id = 3",
            &[],
        )
        .await
        .unwrap()[0]
            .try_get(0)
            .unwrap();
        assert_eq!(stored, "2001-02-03T04:05:06.123456Z");
    }
}

#[tokio::test]
async fn unportable_stored_date_times_fail_the_export() {
    let ddl = events_ddl();
    let source = open("dt-unportable", &[&ddl]).await;
    #[cfg(feature = "sqlite")]
    let bad_values = [
        "'not a date'",
        "'0000-01-01T00:00:00Z'",
        "'0001-01-01T00:00:00+01:00'",
        "'9999-12-31T23:59:59-01:00'",
        "'9999-12-31T23:59:59.9999995Z'",
    ];
    // PostgreSQL infinities and years outside the portable range must fail cleanly, before
    // chrono's binary decode can overflow. PostgreSQL's 1 BC is astronomical year zero.
    #[cfg(feature = "postgres")]
    let bad_values = [
        "'infinity'",
        "'-infinity'",
        "'0001-01-01 00:00:00 BC'",
        "'10000-01-01 00:00:00+00'",
    ];
    for bad in bad_values {
        exec(
            &source.db,
            &format!("INSERT INTO dt_events (id, created_at, label) VALUES (1, {bad}, 'a')"),
        )
        .await
        .unwrap();
        assert!(export(&source.db, &events()).await.is_err(), "{bad}");
        exec(&source.db, "DELETE FROM dt_events").await.unwrap();
    }
}

#[tokio::test]
async fn non_canonical_or_mistyped_snapshot_date_times_are_refused_before_any_write() {
    let ddl = events_ddl();
    let source = open("dt-refuse-source", &[&ddl]).await;
    exec(
        &source.db,
        "INSERT INTO dt_events (id, created_at, label) VALUES (1, '2001-02-03T04:05:06.123456Z', 'a'), (2, '2001-02-03T04:05:07.5Z', 'b')",
    )
    .await
    .unwrap();
    let entities = events();
    let rows = export(&source.db, &entities).await.unwrap();
    let target = open("dt-refuse-target", &[&ddl]).await;
    for replacement in [
        BackupValue::String("2001-02-03T04:05:06Z".into()),
        BackupValue::String("2001-02-03T14:05:06.123456+10:00".into()),
        BackupValue::String("0000-01-01T00:00:00.000000Z".into()),
        BackupValue::String("+10000-01-01T00:00:00.000000Z".into()),
        BackupValue::Integer(981_173_106),
    ] {
        let mut tampered = rows.clone();
        tampered.get_mut("dt_events").unwrap()[1]
            .values
            .insert("created_at".into(), replacement.clone());
        let snapshot = snapshot(&entities);
        for context in [RestoreContext::dry_run(), RestoreContext::empty_database()] {
            assert!(
                target
                    .db
                    .restore_backup_rows(&snapshot, &snapshot, &tampered, &context)
                    .await
                    .is_err(),
                "{replacement:?} in {:?}",
                context.mode
            );
        }
        assert_eq!(count(&target.db, "dt_events").await, 0);
    }
}

fn self_ddl() -> [&'static str; 2] {
    [
        "CREATE TABLE sr_nodes (id BIGINT PRIMARY KEY, parent_id BIGINT NULL REFERENCES sr_nodes(id), label TEXT NOT NULL)",
        "CREATE TABLE sr_ring (id BIGINT PRIMARY KEY, next_id BIGINT NOT NULL REFERENCES sr_ring(id))",
    ]
}
fn self_entities() -> Vec<EntityBackupDescriptor> {
    vec![
        table(
            "sr_nodes",
            vec![
                column("id", BackupValueKind::Integer, false),
                column("parent_id", BackupValueKind::Integer, true),
                column("label", BackupValueKind::String, false),
            ],
            Some("parent_id"),
        ),
        table(
            "sr_ring",
            vec![
                column("id", BackupValueKind::Integer, false),
                column("next_id", BackupValueKind::Integer, false),
            ],
            Some("next_id"),
        ),
    ]
}

#[tokio::test]
async fn self_reference_trees_loops_and_cycles_round_trip_with_foreign_keys_enforced() {
    let ddl = self_ddl();
    let source = open("sr-source", &ddl).await;
    // One multi-row statement per cycle: both backends check immediate foreign keys at the end
    // of the statement.
    for sql in [
        // Nullable: a tree, a self-loop, a 2-cycle, a 3-cycle, and a row below the 3-cycle.
        "INSERT INTO sr_nodes (id, parent_id, label) VALUES (1, NULL, 'root'), (2, 1, 'child'), (3, 2, 'leaf')",
        "INSERT INTO sr_nodes (id, parent_id, label) VALUES (4, 4, 'loop')",
        "INSERT INTO sr_nodes (id, parent_id, label) VALUES (5, 6, 'a'), (6, 5, 'b')",
        "INSERT INTO sr_nodes (id, parent_id, label) VALUES (7, 8, 'x'), (8, 9, 'y'), (9, 7, 'z')",
        "INSERT INTO sr_nodes (id, parent_id, label) VALUES (10, 7, 'below')",
        // Non-null: a self-loop, a 2-cycle, a 3-cycle, and a row below the 2-cycle.
        "INSERT INTO sr_ring (id, next_id) VALUES (1, 1)",
        "INSERT INTO sr_ring (id, next_id) VALUES (2, 3), (3, 2)",
        "INSERT INTO sr_ring (id, next_id) VALUES (4, 5), (5, 6), (6, 4)",
        "INSERT INTO sr_ring (id, next_id) VALUES (7, 2)",
    ] {
        exec(&source.db, sql).await.unwrap();
    }
    let entities = self_entities();
    let rows = export(&source.db, &entities).await.unwrap();

    let target = open("sr-target", &ddl).await;
    // The target enforces foreign keys, so the own-key placeholders below are really checked.
    assert!(
        exec(
            &target.db,
            "INSERT INTO sr_ring (id, next_id) VALUES (100, 999)"
        )
        .await
        .is_err()
    );
    assert_eq!(count(&target.db, "sr_ring").await, 0);

    let reports = restore(&target.db, &entities, &rows).await.unwrap();
    assert_eq!(
        reports.iter().map(|r| r.rows_imported).collect::<Vec<_>>(),
        [10, 7]
    );
    assert_eq!(export(&target.db, &entities).await.unwrap(), rows);
}

#[tokio::test]
async fn unique_self_references_stage_through_null_and_refuse_a_required_unique_cycle() {
    let ddl = [
        // One-to-one, nullable: NULL placeholders never collide.
        "CREATE TABLE sr_pair (id BIGINT PRIMARY KEY, partner_id BIGINT NULL UNIQUE REFERENCES sr_pair(id))",
        // One-to-one, required: a cycle cannot be staged without a temporary value.
        "CREATE TABLE sr_mate (id BIGINT PRIMARY KEY, mate_id BIGINT NOT NULL UNIQUE REFERENCES sr_mate(id))",
    ];
    let source = open("sr-unique-source", &ddl).await;
    for sql in [
        "INSERT INTO sr_pair (id, partner_id) VALUES (1, 2), (2, 1), (3, NULL), (4, 5), (5, 4)",
        "INSERT INTO sr_mate (id, mate_id) VALUES (1, 2), (2, 1)",
    ] {
        exec(&source.db, sql).await.unwrap();
    }
    let pair = vec![table(
        "sr_pair",
        vec![
            column("id", BackupValueKind::Integer, false),
            column("partner_id", BackupValueKind::Integer, true),
        ],
        Some("partner_id"),
    )];
    let mate = vec![table(
        "sr_mate",
        vec![
            column("id", BackupValueKind::Integer, false),
            column("mate_id", BackupValueKind::Integer, false),
        ],
        Some("mate_id"),
    )];

    let pairs = export(&source.db, &pair).await.unwrap();
    let target = open("sr-unique-target", &ddl).await;
    restore(&target.db, &pair, &pairs).await.unwrap();
    assert_eq!(export(&target.db, &pair).await.unwrap(), pairs);

    let mates = export(&source.db, &mate).await.unwrap();
    let error = restore(&target.db, &mate, &mates).await.unwrap_err();
    let cycle = BackupSelfReferenceCycle::from_error(&error).expect("named refusal");
    assert_eq!(cycle.table_name, "sr_mate");
    assert_eq!(cycle.columns, ["mate_id"]);
    // The table's import transaction rolled back.
    assert_eq!(count(&target.db, "sr_mate").await, 0);
}

#[tokio::test]
async fn database_identity_names_the_connected_database() {
    let db = open("identity", &[]).await;
    let identity = db.db.database_identity().await.unwrap();
    #[cfg(feature = "sqlite")]
    {
        assert_eq!(
            std::fs::canonicalize(&identity.name).unwrap(),
            std::fs::canonicalize(&db.path).unwrap()
        );
        assert_eq!(identity.object_id, None);
    }
    #[cfg(feature = "postgres")]
    {
        assert_eq!(
            Some(identity.name.as_str()),
            db.owned.url.rsplit('/').next()
        );
        assert!(identity.object_id.is_some_and(|oid| oid > 0));
    }
}
