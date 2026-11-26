//! Backup adapter conformance against test-owned PostgreSQL databases.

// The shared conformance suite builds `LocalBackupRepository::new`, which does
// not exist on Windows.
#![cfg(not(windows))]

#[allow(dead_code)]
#[path = "../support/owned_postgres.rs"]
mod owned_postgres;

include!("../support/orm_016_conformance.rs");

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn restore_emptiness_refuses_rows_hidden_by_rls() -> Result<(), Box<dyn std::error::Error>> {
    let owner = owned_postgres::OwnedPostgres::pool("backup-empty-rls", 2).await?;
    graphql_orm::sqlx::query("CREATE TABLE conformance_identity_accounts (id uuid PRIMARY KEY, name text, profile jsonb, secret text, ephemeral_token text)").execute(&owner).await?;
    graphql_orm::sqlx::query("INSERT INTO conformance_identity_accounts (id, name) VALUES (pg_catalog.gen_random_uuid(), 'hidden')").execute(&owner).await?;
    graphql_orm::sqlx::query("ALTER TABLE conformance_identity_accounts ENABLE ROW LEVEL SECURITY")
        .execute(&owner)
        .await?;
    graphql_orm::sqlx::query(
        "CREATE POLICY hidden_rows ON conformance_identity_accounts USING (false)",
    )
    .execute(&owner)
    .await?;
    let role = format!("backup_rls_{}", Uuid::new_v4().simple());
    graphql_orm::sqlx::query(&format!("CREATE ROLE {role} NOSUPERUSER NOBYPASSRLS"))
        .execute(&owner)
        .await?;
    graphql_orm::sqlx::query(&format!("GRANT USAGE ON SCHEMA public TO {role}"))
        .execute(&owner)
        .await?;
    graphql_orm::sqlx::query(&format!(
        "GRANT SELECT ON conformance_identity_accounts TO {role}"
    ))
    .execute(&owner)
    .await?;
    let runtime = graphql_orm::sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .after_connect(move |conn, _| {
            let sql = format!("SET ROLE {role}");
            Box::pin(async move {
                graphql_orm::sqlx::query(&sql).execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(owner.connect_options().as_ref().clone())
        .await?;
    let metadata = vec![identity_module::ConformanceAccount::metadata()];
    let owner_adapter = OrmBackupAdapter::new(
        Arc::new(graphql_orm::db::Database::new(owner.clone())),
        metadata.clone(),
    );
    assert!(!owner_adapter.restore_target_is_empty().await?);
    let adapter = OrmBackupAdapter::new(
        Arc::new(graphql_orm::db::Database::new(runtime.clone())),
        metadata,
    );
    let error = adapter
        .restore_target_is_empty()
        .await
        .expect_err("a filtered target count must not look empty");
    assert!(error.to_string().contains("row-level security"), "{error}");
    runtime.close().await;
    owner.close().await;
    Ok(())
}

#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(table = "MixedBackupNotes", plural = "MixedBackupNotes")]
#[allow(dead_code)]
struct MixedBackupNote {
    #[primary_key]
    id: String,
    title: String,
}

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn mixed_case_backup_restore_round_trip() -> Result<(), Box<dyn std::error::Error>> {
    let pool = owned_postgres::OwnedPostgres::pool("backup-table-case", 1).await?;
    let db = Arc::new(graphql_orm::db::Database::new(pool.clone()));
    let plan = db
        .schema()
        .plan_migration_to_entities("mixed-backup", "create", &[MixedBackupNote::metadata()])
        .await?;
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    graphql_orm::sqlx::query("INSERT INTO MixedBackupNotes VALUES ('one', 'preserved')")
        .execute(&pool)
        .await?;
    let adapter = OrmBackupAdapter::new(db, vec![MixedBackupNote::metadata()]);
    let schema = adapter.schema_snapshot().await?;
    let rows = adapter.export_full().await?;
    assert!(!adapter.restore_target_is_empty().await?);
    adapter.clear_restore_target().await?;
    assert!(adapter.restore_target_is_empty().await?);
    adapter
        .restore_full(schema, rows, RestoreContext::empty_database())
        .await?;
    let title: String =
        graphql_orm::sqlx::query_scalar("SELECT title FROM MixedBackupNotes WHERE id = 'one'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(title, "preserved");
    Ok(())
}

#[cfg(feature = "canonical-journal-keys")]
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL and graphql-orm/change-journal; run the backup-postgres owned lane"]
async fn canonical_journal_keys_compact_and_restore_after_cutover()
-> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::graphql::orm::{
        BackupChangeAction as JournalAction, BackupValueKind, ChangeWindow,
    };
    use graphql_orm_backup::{
        BackupChangeAction, BackupChangeExport, CompactChainRequest, IncrementalBackupRequest,
        compact_chain, create_incremental_backup,
    };

    let pool = owned_postgres::OwnedPostgres::pool("backup-canonical-journal", 1).await?;
    // The portable instant descriptor is shared with runtime backup. Bind it
    // explicitly here so the fixture can use a native timestamp primary key.
    let entities: Vec<&'static EntityMetadata> = [
        ("canonical_float", BackupValueKind::Float),
        ("canonical_time", BackupValueKind::DateTime),
        ("canonical_text", BackupValueKind::String),
        ("canonical_bytes", BackupValueKind::Bytes),
    ]
    .into_iter()
    .map(|(table_name, logical_type)| {
        let mut metadata = MixedBackupNote::metadata().clone();
        metadata.table_name = table_name;
        metadata.entity_name = table_name;
        metadata
            .fields
            .iter_mut()
            .find(|field| field.is_primary_key)
            .unwrap()
            .logical_type = logical_type;
        &*Box::leak(Box::new(metadata))
    })
    .collect();
    graphql_orm::sqlx::raw_sql(
        "CREATE COLLATION canonical_ci (provider = icu, locale = 'und-u-ks-level2', deterministic = false);
         CREATE TABLE canonical_float (id double precision PRIMARY KEY, title text NOT NULL);
         CREATE TABLE canonical_time (id timestamptz PRIMARY KEY, title text NOT NULL);
         CREATE TABLE canonical_text (id text COLLATE canonical_ci PRIMARY KEY, title text NOT NULL);
         CREATE TABLE canonical_bytes (id bytea PRIMARY KEY, title text NOT NULL);
         SET DateStyle = 'ISO, MDY';
         SET TimeZone = 'UTC';
         SET extra_float_digits = 1;",
    )
    .execute(&pool)
    .await?;
    let db = Arc::new(graphql_orm::db::Database::new(pool.clone()));
    db.ensure_change_journal_recorder(&entities).await?;
    graphql_orm::sqlx::raw_sql(
        "SET graphql_orm.change_journal = 'on';
         INSERT INTO canonical_float VALUES (1.2345678901234567, 'retained'), (1.2345678901234569, 'deleted');
         INSERT INTO canonical_time VALUES ('2026-11-09 12:34:56.123456+00', 'retained'), ('2026-11-09 12:34:57+00', 'deleted');
         INSERT INTO canonical_text VALUES ('École', 'original');
         INSERT INTO canonical_bytes VALUES (pg_catalog.decode('00ff', 'hex'), 'retained'), (pg_catalog.decode('00fe', 'hex'), 'deleted');",
    )
    .execute(&pool)
    .await?;

    let temp = TempDir::new()?;
    let repo = LocalBackupRepository::new(temp.path())
        .with_manifest_authenticator(test_manifest_authenticator());
    let objects = NoJournalObjects;
    let legacy = OrmBackupAdapter::new(db.clone(), entities.clone());
    let full_request = |snapshot_id| FullBackupRequest {
        snapshot_id,
        created_at: 1,
        app_id: "canonical-journal".into(),
        app_version: "1.0".into(),
    };
    let legacy_id = Uuid::new_v4();
    create_full_backup(&repo, &legacy, &objects, full_request(legacy_id)).await?;
    let legacy_position = db
        .export_changes(ChangeWindow {
            after_sequence: None,
            until: i64::MAX,
        })
        .await?
        .last()
        .unwrap()
        .sequence;
    let start = db.enable_canonical_change_journal_keys().await?;
    let orm = OrmBackupAdapter::new(db.clone(), entities).with_canonical_change_journal_keys();
    let incremental = |parent_snapshot_id| IncrementalBackupRequest {
        snapshot_id: Uuid::new_v4(),
        parent_snapshot_id,
        created_at: 2,
        app_id: "canonical-journal".into(),
        app_version: "1.0".into(),
    };
    let error = create_incremental_backup(&repo, &orm, &objects, incremental(legacy_id))
        .await
        .unwrap_err();
    assert!(matches!(error, BackupError::InvalidManifestChain { .. }));
    assert!(
        error.to_string().contains("start a new full backup"),
        "{error}"
    );
    let error = db
        .export_changes(ChangeWindow {
            after_sequence: Some(legacy_position),
            until: i64::MAX,
        })
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("predates the canonical key cutover"),
        "{error}"
    );

    let full_id = Uuid::new_v4();
    create_full_backup(&repo, &orm, &objects, full_request(full_id)).await?;
    let original = orm.export_full().await?;
    graphql_orm::sqlx::raw_sql(
        "SET DateStyle = 'SQL, DMY';
         SET TimeZone = 'Asia/Tokyo';
         SET extra_float_digits = -2;
         SET bytea_output = 'escape';",
    )
    .execute(&pool)
    .await?;
    assert_eq!(original, orm.export_full().await?);
    // Record under a different wire encoding, then return to UTF-8 so SQLx
    // can read the application text.
    graphql_orm::sqlx::raw_sql(
        "SET client_encoding = 'LATIN1';
         DELETE FROM canonical_float WHERE title = 'deleted';
         DELETE FROM canonical_time WHERE title = 'deleted';
         DELETE FROM canonical_bytes WHERE title = 'deleted';
         UPDATE canonical_text SET id = pg_catalog.chr(233) || 'COLE', title = 'updated';
         SET client_encoding = 'UTF8';",
    )
    .execute(&pool)
    .await?;
    let current = orm.export_full().await?;
    let journal = db
        .export_changes(ChangeWindow {
            after_sequence: Some(start),
            until: i64::MAX,
        })
        .await?;
    assert_eq!(journal.len(), 5);
    let changes = journal
        .into_iter()
        .map(|entry| {
            let action = match entry.action {
                JournalAction::Create => BackupChangeAction::Create,
                JournalAction::Update => BackupChangeAction::Update,
                JournalAction::Delete => BackupChangeAction::Delete,
            };
            let tables = if action == BackupChangeAction::Delete {
                &original
            } else {
                &current
            };
            let row = tables
                .iter()
                .find(|table| table.table_name == entry.table_name)
                .unwrap()
                .rows
                .iter()
                .find(|row| row.primary_key == entry.primary_key)
                .expect("journal key must match the exported identity")
                .clone();
            let row = (action != BackupChangeAction::Delete).then_some(row);
            BackupChangeExport {
                table_name: entry.table_name,
                primary_key: entry.primary_key,
                action,
                row,
                changed_at: entry.changed_at,
            }
        })
        .collect();
    let source = FixtureJournalSource {
        orm: &orm,
        changes,
        retype_during_export: None,
    };
    let request = incremental(full_id);
    let delta_id = request.snapshot_id;
    create_incremental_backup(&repo, &source, &objects, request).await?;
    let compacted_id = Uuid::new_v4();
    let compacted = compact_chain(
        &repo,
        CompactChainRequest {
            snapshot_id: compacted_id,
            source_snapshot_id: delta_id,
            created_at: 3,
            app_id: "canonical-journal".into(),
            app_version: "1.0".into(),
        },
    )
    .await?;
    assert_eq!(compacted.manifest.database.row_count, 4);

    orm.clear_restore_target().await?;
    restore_snapshot(&repo, &orm, compacted_id, RestoreContext::empty_database()).await?;
    assert_eq!(orm.export_full().await?, current);
    Ok(())
}

#[cfg(feature = "canonical-journal-keys")]
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL and graphql-orm/change-journal; run the backup-postgres owned lane"]
async fn canonical_chain_refuses_a_live_key_type_change() -> Result<(), Box<dyn std::error::Error>>
{
    use graphql_orm::graphql::orm::BackupValueKind;
    use graphql_orm_backup::{
        BackupChangeAction, BackupChangeExport, IncrementalBackupRequest, create_incremental_backup,
    };

    let pool = owned_postgres::OwnedPostgres::pool("backup-key-storage-change", 1).await?;
    graphql_orm::sqlx::query(
        "CREATE TABLE key_storage_change (id int4 PRIMARY KEY, title text NOT NULL)",
    )
    .execute(&pool)
    .await?;
    let mut metadata = MixedBackupNote::metadata().clone();
    metadata.table_name = "key_storage_change";
    metadata
        .fields
        .iter_mut()
        .find(|field| field.is_primary_key)
        .unwrap()
        .logical_type = BackupValueKind::Integer;
    let entities: Vec<&'static EntityMetadata> = vec![Box::leak(Box::new(metadata))];
    let db = Arc::new(graphql_orm::db::Database::new(pool.clone()));
    db.ensure_change_journal_recorder(&entities).await?;
    db.enable_canonical_change_journal_keys().await?;
    let adapter = OrmBackupAdapter::new(db, entities.clone()).with_canonical_change_journal_keys();
    let directory = TempDir::new()?;
    let repository = LocalBackupRepository::new(directory.path())
        .with_manifest_authenticator(test_manifest_authenticator());
    let parent = Uuid::new_v4();
    // The portable integer row decoder expects int8. An empty int4 table
    // still contributes its storage type to the full backup's identity.
    create_full_backup(
        &repository,
        &adapter,
        &NoJournalObjects,
        FullBackupRequest {
            snapshot_id: parent,
            created_at: 1,
            app_id: "key-storage-change".into(),
            app_version: "1".into(),
        },
    )
    .await?;
    let during_export = FixtureJournalSource {
        orm: &adapter,
        changes: vec![],
        retype_during_export: Some(&pool),
    };
    let attempted = Uuid::new_v4();
    let error = create_incremental_backup(
        &repository,
        &during_export,
        &NoJournalObjects,
        IncrementalBackupRequest {
            snapshot_id: attempted,
            parent_snapshot_id: parent,
            created_at: 2,
            app_id: "key-storage-change".into(),
            app_version: "1".into(),
        },
    )
    .await
    .expect_err("the post-export recheck must detect key DDL");
    assert!(
        error.to_string().contains("changed during export"),
        "{error}"
    );
    assert!(
        graphql_orm_backup::load_manifest(&repository, attempted)
            .await
            .is_err()
    );

    graphql_orm::sqlx::raw_sql(
        "SET graphql_orm.change_journal = 'on';
         INSERT INTO key_storage_change VALUES (1, 'new row');",
    )
    .execute(&pool)
    .await?;
    // Reconnect after the DDL so a cached full-export SQL plan from the old
    // column type cannot mask the manifest-chain check.
    let reconnected = graphql_orm::sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(pool.connect_options().as_ref().clone())
        .await?;
    let db = Arc::new(graphql_orm::db::Database::new(reconnected));
    let adapter = OrmBackupAdapter::new(db, entities).with_canonical_change_journal_keys();
    let row = adapter
        .export_full()
        .await?
        .pop()
        .unwrap()
        .rows
        .pop()
        .unwrap();
    let source = FixtureJournalSource {
        orm: &adapter,
        retype_during_export: None,
        changes: vec![BackupChangeExport {
            table_name: row.table_name.clone(),
            primary_key: row.primary_key.clone(),
            action: BackupChangeAction::Create,
            row: Some(row),
            changed_at: 2,
        }],
    };
    let error = create_incremental_backup(
        &repository,
        &source,
        &NoJournalObjects,
        IncrementalBackupRequest {
            snapshot_id: Uuid::new_v4(),
            parent_snapshot_id: parent,
            created_at: 2,
            app_id: "key-storage-change".into(),
            app_version: "1".into(),
        },
    )
    .await
    .expect_err("changing a live primary-key type must break the identity of the chain");
    assert!(matches!(error, BackupError::InvalidManifestChain { .. }));
    assert!(
        error.to_string().contains("start a new full backup"),
        "{error}"
    );
    Ok(())
}

#[cfg(feature = "canonical-journal-keys")]
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL and graphql-orm/change-journal; run the backup-postgres owned lane"]
async fn canonical_restore_ignores_registrations_outside_the_backup()
-> Result<(), Box<dyn std::error::Error>> {
    let source = owned_postgres::OwnedPostgres::pool("canonical-restore-source", 1).await?;
    let target = owned_postgres::OwnedPostgres::pool("canonical-restore-target", 1).await?;
    let entity = MixedBackupNote::metadata();
    let mut extra = entity.clone();
    extra.table_name = "retired_notes";
    extra.entity_name = "RetiredNote";
    let extra = Box::leak(Box::new(extra));
    for pool in [&source, &target] {
        graphql_orm::sqlx::query(
            "CREATE TABLE MixedBackupNotes (id text PRIMARY KEY, title text NOT NULL)",
        )
        .execute(pool)
        .await?;
    }
    graphql_orm::sqlx::raw_sql(
        "CREATE TABLE retired_notes (id text PRIMARY KEY, title text NOT NULL);
         INSERT INTO MixedBackupNotes VALUES ('note', 'restored');",
    )
    .execute(&source)
    .await?;
    let source = Arc::new(graphql_orm::db::Database::new(source));
    let target_db = Arc::new(graphql_orm::db::Database::new(target.clone()));
    source
        .ensure_change_journal_recorder(&[entity, extra])
        .await?;
    target_db.ensure_change_journal_recorder(&[entity]).await?;
    source.enable_canonical_change_journal_keys().await?;
    target_db.enable_canonical_change_journal_keys().await?;
    let source = OrmBackupAdapter::new(source, vec![entity]).with_canonical_change_journal_keys();
    let target_adapter =
        OrmBackupAdapter::new(target_db, vec![entity]).with_canonical_change_journal_keys();

    let directory = TempDir::new()?;
    let repository = LocalBackupRepository::new(directory.path())
        .with_manifest_authenticator(test_manifest_authenticator());
    let snapshot_id = Uuid::new_v4();
    create_full_backup(
        &repository,
        &source,
        &NoJournalObjects,
        FullBackupRequest {
            snapshot_id,
            created_at: 1,
            app_id: "canonical-restore".into(),
            app_version: "1".into(),
        },
    )
    .await?;
    restore_snapshot(
        &repository,
        &target_adapter,
        snapshot_id,
        RestoreContext::empty_database(),
    )
    .await
    .expect("an unrelated recorder must not affect restore");
    assert_eq!(
        source.export_full().await?,
        target_adapter.export_full().await?
    );

    graphql_orm::sqlx::query("ALTER TABLE MixedBackupNotes ALTER COLUMN id TYPE varchar(12)")
        .execute(&target)
        .await?;
    let mismatch = restore_snapshot(
        &repository,
        &target_adapter,
        snapshot_id,
        RestoreContext::empty_database(),
    )
    .await
    .unwrap_err();
    let mismatch = std::error::Error::source(&mismatch).unwrap().to_string();
    assert!(
        mismatch.contains("MixedBackupNotes") && mismatch.contains("column \"id\""),
        "{mismatch}"
    );
    assert!(!mismatch.contains("restored"), "{mismatch}");

    graphql_orm::sqlx::query("ALTER TABLE MixedBackupNotes RENAME COLUMN id TO old_id")
        .execute(&target)
        .await?;
    let missing = target_adapter
        .schema_snapshot()
        .await
        .unwrap_err()
        .to_string();
    assert!(
        missing.contains("table or column not found") && missing.contains("mixedbackupnotes"),
        "{missing}"
    );
    Ok(())
}

// The fixture freezes writers after the tested change batch, then supplies
// the resulting row bodies and tombstones to the existing host contract.
#[cfg(feature = "canonical-journal-keys")]
struct FixtureJournalSource<'a> {
    orm: &'a OrmBackupAdapter,
    changes: Vec<graphql_orm_backup::BackupChangeExport>,
    retype_during_export: Option<&'a graphql_orm::sqlx::PgPool>,
}

#[cfg(feature = "canonical-journal-keys")]
#[async_trait]
impl GraphqlOrmBackupAdapter for FixtureJournalSource<'_> {
    async fn schema_snapshot(
        &self,
    ) -> Result<graphql_orm_backup::GraphqlOrmBackupSchema, BackupError> {
        self.orm.schema_snapshot().await
    }

    async fn export_full(&self) -> Result<Vec<BackupTableExport>, BackupError> {
        self.orm.export_full().await
    }

    async fn export_incremental(
        &self,
        _: Uuid,
    ) -> Result<Vec<graphql_orm_backup::BackupChangeExport>, BackupError> {
        if let Some(pool) = self.retype_during_export {
            graphql_orm::sqlx::query("ALTER TABLE key_storage_change ALTER COLUMN id TYPE int8")
                .execute(pool)
                .await
                .map_err(|error| BackupError::Database {
                    message: error.to_string(),
                })?;
        }
        Ok(self.changes.clone())
    }

    async fn restore_full(
        &self,
        schema: graphql_orm_backup::GraphqlOrmBackupSchema,
        rows: Vec<BackupTableExport>,
        context: RestoreContext,
    ) -> Result<(), BackupError> {
        self.orm.restore_full(schema, rows, context).await
    }

    async fn restore_incremental(
        &self,
        changes: Vec<graphql_orm_backup::BackupChangeExport>,
        context: RestoreContext,
    ) -> Result<(), BackupError> {
        self.orm.restore_incremental(changes, context).await
    }
}

#[cfg(feature = "canonical-journal-keys")]
struct NoJournalObjects;

#[cfg(feature = "canonical-journal-keys")]
#[async_trait]
impl BackupObjectIndex for NoJournalObjects {
    async fn objects_from_full_export(
        &self,
        _: &[BackupTableExport],
    ) -> Result<Vec<BackupObjectRef>, BackupError> {
        Ok(Vec::new())
    }

    async fn list_objects_for_full_backup(&self) -> Result<Vec<BackupObjectRef>, BackupError> {
        Ok(Vec::new())
    }

    async fn objects_from_incremental_export(
        &self,
        _: Uuid,
        _: &[graphql_orm_backup::BackupChangeExport],
    ) -> Result<Vec<BackupObjectRef>, BackupError> {
        Ok(Vec::new())
    }

    async fn list_objects_for_incremental_backup(
        &self,
        _: Uuid,
    ) -> Result<Vec<BackupObjectRef>, BackupError> {
        Ok(Vec::new())
    }

    async fn load_object(&self, _: &BackupObjectRef) -> Result<Bytes, BackupError> {
        unreachable!("fixture has no objects")
    }
}
