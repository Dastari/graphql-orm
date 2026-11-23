use graphql_orm::graphql::orm::{ColumnBackupPolicy, SchemaModuleCatalog};
use graphql_orm_ai::{AI_SCHEMA_MODULE_VERSION, AI_TABLE_NAMESPACE, AiSchemaModule};

#[test]
fn ai_schema_module_owns_only_reserved_namespace_tables() {
    let module = AiSchemaModule;
    let catalog = SchemaModuleCatalog::compose(&[&module]).expect("AI module should validate");

    assert_eq!(catalog.modules().len(), 1);
    assert_eq!(catalog.modules()[0].version, AI_SCHEMA_MODULE_VERSION);
    assert_eq!(AI_SCHEMA_MODULE_VERSION, "0.85.0");
    assert_eq!(catalog.entities().len(), 50);
    // The stored-response ledger keeps its identifiers in backups so a
    // restored deletion debt is not lost.
    let ledger = catalog
        .entities()
        .iter()
        .find(|entity| entity.table_name == "graphql_orm_ai_stored_provider_responses")
        .expect("stored-response ledger should be module-owned");
    assert_eq!(
        ledger
            .fields
            .iter()
            .find(|field| field.name == "provider_response_id")
            .expect("response identifier column")
            .backup_policy,
        ColumnBackupPolicy::Include
    );
    assert!(
        catalog
            .entities()
            .iter()
            .all(|entity| entity.table_name.starts_with(AI_TABLE_NAMESPACE))
    );
    assert!(
        catalog
            .entities()
            .iter()
            .filter(|entity| matches!(
                entity.table_name,
                "graphql_orm_ai_run_attempts"
                    | "graphql_orm_ai_run_attempt_outcomes"
                    | "graphql_orm_ai_run_checkpoints"
                    | "graphql_orm_ai_skill_versions"
                    | "graphql_orm_ai_usage_entries"
                    | "graphql_orm_ai_pricing_policies"
                    | "graphql_orm_ai_audit_events"
                    | "graphql_orm_ai_egress_events"
            ))
            .all(|entity| entity.append_only)
    );
    let retention_entities = catalog
        .entities()
        .iter()
        .filter(|entity| entity.retention_policy.is_some())
        .collect::<Vec<_>>();
    if cfg!(feature = "mssql") {
        assert!(retention_entities.is_empty());
    } else {
        assert_eq!(retention_entities.len(), 1);
        assert_eq!(
            retention_entities[0].table_name,
            "graphql_orm_ai_run_checkpoints"
        );
        assert_eq!(
            retention_entities[0].retention_policy,
            Some("graphql_orm_ai.run_checkpoint.retention_purge")
        );
    }
    assert_eq!(catalog.modules()[0].restore_hooks.len(), 4);

    let schema = catalog.schema_model();
    let native_candidate = schema
        .tables
        .iter()
        .find(|table| table.table_name == "graphql_orm_ai_native_approval_candidates")
        .expect("protected native approval candidate table should exist");
    assert!(native_candidate.columns.iter().any(|column| {
        column.name == "budget_reservation_id" && column.is_unique && !column.nullable
    }));
    for protected in ["protected_preparation", "protected_control_receipt"] {
        assert!(
            native_candidate
                .columns
                .iter()
                .any(|column| { column.name == protected && column.nullable })
        );
        let metadata = catalog
            .entities()
            .iter()
            .find(|entity| entity.table_name == native_candidate.table_name)
            .unwrap();
        assert_eq!(
            metadata
                .fields
                .iter()
                .find(|field| field.name == protected)
                .unwrap()
                .backup_policy,
            ColumnBackupPolicy::Include
        );
    }
    let tool_call = schema
        .tables
        .iter()
        .find(|table| table.table_name == "graphql_orm_ai_tool_calls")
        .unwrap();
    assert!(
        tool_call
            .columns
            .iter()
            .any(|column| { column.name == "execution_provenance" && column.nullable })
    );
    let counter = schema
        .tables
        .iter()
        .find(|table| table.table_name == "graphql_orm_ai_budget_counters")
        .expect("budget counter table should exist");
    let budget_policy = schema
        .tables
        .iter()
        .find(|table| table.table_name == "graphql_orm_ai_budget_policies")
        .expect("budget policy table should exist");
    assert!(
        budget_policy
            .columns
            .iter()
            .any(|column| { column.name == "scope_key" && !column.nullable })
    );
    let pricing_policy = schema
        .tables
        .iter()
        .find(|table| table.table_name == "graphql_orm_ai_pricing_policies")
        .expect("pricing policy table should exist");
    assert!(pricing_policy.append_only);
    assert!(pricing_policy.columns.iter().any(|column| {
        column.name == "version_reference" && column.is_unique && !column.nullable
    }));
    for expected in ["scope_key", "provider_kind", "provider_model"] {
        assert!(
            pricing_policy
                .indexes
                .iter()
                .any(|index| { index.columns == [expected.to_owned()] && !index.is_unique })
        );
    }
    assert!(
        budget_policy
            .indexes
            .iter()
            .any(|index| { index.columns == ["scope_key"] && !index.is_unique })
    );
    assert!(
        counter.composite_unique_indexes.iter().any(|columns| {
            columns == &["budget_policy_id".to_owned(), "period_key".to_owned()]
        })
    );
    let reservation = schema
        .tables
        .iter()
        .find(|table| table.table_name == "graphql_orm_ai_budget_reservations")
        .expect("budget reservation table should exist");
    assert!(reservation.composite_unique_indexes.iter().any(|columns| {
        columns
            == &[
                "principal_kind".to_owned(),
                "principal_subject".to_owned(),
                "idempotency_key".to_owned(),
            ]
    }));
    assert!(reservation.indexes.iter().any(|index| {
        index.name == "idx_graphql_orm_ai_budget_reservations_scope_state"
            && index.columns == ["scope_kind", "scope_id", "tenant_id", "state", "expires_at"]
            && !index.is_unique
    }));
    assert!(
        reservation
            .columns
            .iter()
            .any(|column| { column.name == "actual_cached_input_tokens" && column.nullable })
    );
    let inbox_stream = schema
        .tables
        .iter()
        .find(|table| table.table_name == "graphql_orm_ai_inbox_streams")
        .expect("principal inbox stream table should exist");
    assert!(inbox_stream.composite_unique_indexes.iter().any(|columns| {
        columns == &["principal_kind".to_owned(), "principal_subject".to_owned()]
    }));
    let inbox_event = schema
        .tables
        .iter()
        .find(|table| table.table_name == "graphql_orm_ai_inbox_events")
        .expect("principal inbox event table should exist");
    assert!(inbox_event.composite_unique_indexes.iter().any(|columns| {
        columns
            == &[
                "principal_kind".to_owned(),
                "principal_subject".to_owned(),
                "sequence".to_owned(),
            ]
    }));
    let message = schema
        .tables
        .iter()
        .find(|table| table.table_name == "graphql_orm_ai_messages")
        .expect("message table should exist");
    assert!(
        message
            .columns
            .iter()
            .any(|column| { column.name == "protected_preview" && column.nullable })
    );
    assert!(
        message
            .columns
            .iter()
            .any(|column| { column.name == "content_purged_at" && column.nullable })
    );
    assert!(
        message
            .columns
            .iter()
            .any(|column| { column.name == "row_version" && !column.nullable })
    );
    let attachment = schema
        .tables
        .iter()
        .find(|table| table.table_name == "graphql_orm_ai_attachments")
        .expect("attachment table should exist");
    assert!(
        attachment
            .indexes
            .iter()
            .any(|index| { index.columns == ["message_id"] && !index.is_unique })
    );
    let attachment_metadata = catalog
        .entities()
        .iter()
        .find(|entity| entity.table_name == "graphql_orm_ai_attachments")
        .expect("attachment metadata should exist");
    assert_eq!(
        attachment_metadata
            .fields
            .iter()
            .find(|field| field.name == "blob_reference")
            .expect("attachment blob reference should exist")
            .backup_policy,
        ColumnBackupPolicy::Include
    );
    for redacted in ["quarantine_blob_reference", "upload_token_hash"] {
        assert_eq!(
            attachment_metadata
                .fields
                .iter()
                .find(|field| field.name == redacted)
                .expect("redacted attachment field should exist")
                .backup_policy,
            ColumnBackupPolicy::Redact
        );
    }
    let artifact_metadata = catalog
        .entities()
        .iter()
        .find(|entity| entity.table_name == "graphql_orm_ai_attachment_artifacts")
        .expect("attachment artifact metadata should exist");
    assert_eq!(
        artifact_metadata
            .fields
            .iter()
            .find(|field| field.name == "blob_reference")
            .expect("artifact blob reference should exist")
            .backup_policy,
        ColumnBackupPolicy::Include
    );
    assert_eq!(
        artifact_metadata
            .fields
            .iter()
            .find(|field| field.name == "provider_reference")
            .expect("artifact provider reference should exist")
            .backup_policy,
        ColumnBackupPolicy::Redact
    );
    assert!(
        inbox_event
            .columns
            .iter()
            .any(|column| column.name == "scope_key" && !column.nullable)
    );
    let retention = schema
        .tables
        .iter()
        .find(|table| table.table_name == "graphql_orm_ai_retention_policies")
        .expect("retention policy table should exist");
    assert!(
        retention
            .columns
            .iter()
            .any(|column| { column.name == "scope_key" && column.nullable && !column.is_unique })
    );
    let usage = schema
        .tables
        .iter()
        .find(|table| table.table_name == "graphql_orm_ai_usage_entries")
        .expect("usage table should exist");
    assert!(usage.columns.iter().any(|column| {
        column.name == "budget_reservation_id" && column.is_unique && !column.nullable
    }));
    assert!(
        usage
            .columns
            .iter()
            .any(|column| column.name == "principal_kind" && !column.nullable)
    );
    let webhook_receipt = schema
        .tables
        .iter()
        .find(|table| table.table_name == "graphql_orm_ai_provider_webhook_receipts")
        .expect("provider webhook receipt table should exist");
    assert_eq!(webhook_receipt.primary_keys(), ["id", "provider_kind"]);
    assert!(webhook_receipt.indexes.iter().any(|index| {
        index.columns
            == [
                "provider_kind".to_owned(),
                "provider_profile_id".to_owned(),
                "provider_response_id".to_owned(),
                "state".to_owned(),
            ]
            && !index.is_unique
    }));
    for required_column in [
        "receipt_key",
        "provider_profile_id",
        "provider_event_kind",
        "provider_created_at",
    ] {
        assert!(
            webhook_receipt
                .columns
                .iter()
                .any(|column| { column.name == required_column && !column.nullable })
        );
    }
    let background_submission = schema
        .tables
        .iter()
        .find(|table| table.table_name == "graphql_orm_ai_provider_background_submissions")
        .expect("provider background submission table should exist");
    for required_column in [
        "reconciliation_owner",
        "reconciliation_generation",
        "reconciliation_lease_expires_at",
        "reconciliation_next_attempt_at",
        "reconciliation_retry_count",
        "reconciliation_deadline",
        "reconciled_at",
        "retrieval_egress_decision_id",
        "terminal_message_id",
    ] {
        assert!(
            background_submission
                .columns
                .iter()
                .any(|column| column.name == required_column)
        );
    }
    for indexed_column in [
        "reconciliation_lease_expires_at",
        "reconciliation_next_attempt_at",
        "reconciliation_deadline",
    ] {
        assert!(
            background_submission
                .indexes
                .iter()
                .any(|index| index.columns == [indexed_column] && !index.is_unique)
        );
    }
    for indexed_column in [
        "scope_kind",
        "scope_id",
        "tenant_id",
        "principal_kind",
        "principal_subject",
        "provider_kind",
        "provider_model",
        "created_at",
    ] {
        assert!(
            usage
                .indexes
                .iter()
                .any(|index| { index.columns == [indexed_column] && !index.is_unique })
        );
    }
    assert!(
        retention
            .indexes
            .iter()
            .any(|index| { index.is_unique && index.columns == ["scope_key"] })
    );
    assert!(
        retention
            .columns
            .iter()
            .any(|column| { column.name == "inbox_event_retention_seconds" && column.nullable })
    );
    let attempt_outcome = schema
        .tables
        .iter()
        .find(|table| table.table_name == "graphql_orm_ai_run_attempt_outcomes")
        .expect("run attempt outcome table should exist");
    assert!(
        attempt_outcome
            .columns
            .iter()
            .any(|column| column.name == "attempt_id" && column.is_unique)
    );
    let run = schema
        .tables
        .iter()
        .find(|table| table.table_name == "graphql_orm_ai_runs")
        .expect("run table should exist");
    assert!(
        run.columns
            .iter()
            .any(|column| column.name == "latest_checkpoint_id" && column.nullable)
    );
    let checkpoint = schema
        .tables
        .iter()
        .find(|table| table.table_name == "graphql_orm_ai_run_checkpoints")
        .expect("run checkpoint table should exist");
    assert_eq!(checkpoint.retention_purge, !cfg!(feature = "mssql"));
    assert!(
        checkpoint
            .columns
            .iter()
            .any(|column| column.name == "protected_state" && column.nullable)
    );
}

#[test]
fn ai_schema_identifiers_fit_postgres_identifier_limit() {
    let module = AiSchemaModule;
    let catalog = SchemaModuleCatalog::compose(&[&module]).expect("AI module should validate");
    let schema = catalog.schema_model();
    let mut names = Vec::new();
    for table in &schema.tables {
        names.push(table.table_name.clone());
        names.extend(table.columns.iter().map(|column| column.name.clone()));
        names.extend(table.indexes.iter().map(|index| index.name.to_owned()));
        names.extend(
            table
                .check_constraints
                .iter()
                .map(|check| check.name.to_string()),
        );
        names.extend(table.search_indexes.iter().map(|index| index.name.clone()));
        names.extend(
            table
                .foreign_keys
                .iter()
                .filter_map(|foreign_key| foreign_key.constraint_name.clone()),
        );
    }
    let too_long = names
        .into_iter()
        .filter(|name| name.len() > 63)
        .collect::<Vec<_>>();
    assert!(
        too_long.is_empty(),
        "identifiers over 63 bytes: {too_long:?}"
    );
    let receipts = schema
        .tables
        .iter()
        .find(|table| table.table_name == "graphql_orm_ai_provider_webhook_receipts")
        .expect("webhook receipts table");
    assert!(receipts.indexes.iter().any(|index| {
        index
            .name
            .starts_with("idx_graphql_orm_ai_provider_webhook_receipts_p_")
            && index.name.len() == 63
    }));
}

/// Schema module 0.82.0 adds three unique indexes. A store at 0.81.0 whose rows
/// already repeat one of those keys is refused with a typed error before any
/// migration statement runs, and upgrades once the repeated row is resolved.
#[cfg(feature = "sqlite")]
mod unique_constraint_upgrade {
    use graphql_orm::graphql::orm::{
        ApplyOptions, MigrationStep, OrmSchemaModule, SchemaModuleCatalog, UniqueIndexDuplicates,
    };
    use graphql_orm::prelude::{Database, SqliteBackend};
    use graphql_orm::uuid::Uuid;
    use graphql_orm_ai::AiSchemaModule;

    const CHECKPOINTS: &str = "graphql_orm_ai_context_checkpoints";

    /// The added unique indexes, as (table, key columns).
    const ADDED: [(&str, &[&str]); 3] = [
        ("graphql_orm_ai_approvals", &["tool_call_id"]),
        (
            "graphql_orm_ai_run_attempts",
            &["run_id", "lease_generation"],
        ),
        (CHECKPOINTS, &["session_id", "through_sequence"]),
    ];

    fn added_index_names() -> Vec<String> {
        let module = AiSchemaModule;
        let catalog = SchemaModuleCatalog::compose(&[&module]).expect("AI module should compose");
        let model = catalog.schema_model();
        ADDED
            .iter()
            .map(|(table, columns)| {
                model
                    .tables
                    .iter()
                    .find(|candidate| candidate.table_name == *table)
                    .and_then(|table| {
                        table
                            .indexes
                            .iter()
                            .find(|index| index.is_unique && index.columns == *columns)
                    })
                    .map(|index| index.name.to_owned())
                    .unwrap_or_else(|| panic!("{table} should declare a unique index"))
            })
            .collect()
    }

    async fn existing_indexes(database: &Database<SqliteBackend>, names: &[String]) -> Vec<String> {
        let mut found = Vec::new();
        for name in names {
            let present = graphql_orm::sqlx::query_as::<_, (i64,)>(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?",
            )
            .bind(name)
            .fetch_one(database.pool())
            .await
            .expect("index catalogue should read")
            .0;
            if present == 1 {
                found.push(name.clone());
            }
        }
        found
    }

    async fn insert_checkpoint(
        database: &Database<SqliteBackend>,
        session_id: Uuid,
        through_sequence: i64,
    ) -> Result<String, graphql_orm::sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        graphql_orm::sqlx::query(&format!(
            "INSERT INTO {CHECKPOINTS} (id, session_id, through_sequence, source_hash, \
             token_estimate, provider_kind, provider_model, protected_summary) \
             VALUES (?, ?, ?, 'source', 1, 'mock', 'upgrade-test', '{{}}')"
        ))
        .bind(&id)
        .bind(session_id.to_string())
        .bind(through_sequence)
        .execute(database.pool())
        .await
        .map(|_| id)
    }

    #[tokio::test]
    async fn upgrade_refuses_existing_duplicate_keys_before_any_ddl_then_applies_once_clean() {
        let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
            .await
            .expect("in-memory SQLite should open");
        let module = AiSchemaModule;
        let initial = database
            .schema()
            .plan_migration_to_entities("ai-unique-v1", "AI module", module.entities())
            .await
            .expect("AI module should plan");
        database
            .schema()
            .apply_migration(&initial, ApplyOptions::default())
            .await
            .expect("AI module should apply");

        // Reproduce a schema module 0.81.0 store: the three indexes are absent
        // and one of the keys already repeats.
        let names = added_index_names();
        for name in &names {
            graphql_orm::sqlx::query(&format!("DROP INDEX \"{name}\""))
                .execute(database.pool())
                .await
                .expect("added unique index should drop");
        }
        let session_id = Uuid::new_v4();
        insert_checkpoint(&database, session_id, 7)
            .await
            .expect("first checkpoint should insert");
        let duplicate = insert_checkpoint(&database, session_id, 7)
            .await
            .expect("a 0.81.0 store accepts the repeated key");
        insert_checkpoint(&database, session_id, 8)
            .await
            .expect("a distinct key should insert");

        let upgrade = database
            .schema()
            .plan_migration_to_entities("ai-unique-v2", "unique indexes", module.entities())
            .await
            .expect("upgrade should plan");
        assert_eq!(upgrade.steps.len(), 3, "{:?}", upgrade.statements);
        for (table, columns) in ADDED {
            assert!(upgrade.steps.iter().any(|step| matches!(
                &step.step,
                MigrationStep::CreateIndex { table_name, index }
                    if table_name == table && index.is_unique && index.columns == columns
            )));
        }

        let refused = database
            .schema()
            .apply_migration(&upgrade, ApplyOptions::default())
            .await
            .expect_err("repeated keys must refuse the upgrade");
        let duplicates = UniqueIndexDuplicates::from_error(&refused)
            .unwrap_or_else(|| panic!("refusal should be typed: {refused:?}"));
        assert_eq!(duplicates.table, CHECKPOINTS);
        assert_eq!(duplicates.columns, ["session_id", "through_sequence"]);
        assert!(names.contains(&duplicates.index));
        // Nothing ran: no index was created, including those on clean tables,
        // and the version was not recorded.
        assert_eq!(
            existing_indexes(&database, &names).await,
            Vec::<String>::new()
        );
        assert_eq!(
            database
                .schema()
                .current_version()
                .await
                .expect("version should read"),
            Some("ai-unique-v1".to_owned())
        );

        // The host resolves the duplicate and applies the same plan.
        let removed = graphql_orm::sqlx::query(&format!("DELETE FROM {CHECKPOINTS} WHERE id = ?"))
            .bind(&duplicate)
            .execute(database.pool())
            .await
            .expect("duplicate checkpoint should delete")
            .rows_affected();
        assert_eq!(removed, 1);
        database
            .schema()
            .apply_migration(&upgrade, ApplyOptions::default())
            .await
            .expect("a clean store should upgrade");
        let mut created = existing_indexes(&database, &names).await;
        created.sort();
        let mut expected = names.clone();
        expected.sort();
        assert_eq!(created, expected);
        assert!(
            database
                .schema()
                .plan_migration_to_entities("ai-unique-v3", "replan", module.entities())
                .await
                .expect("replan should succeed")
                .steps
                .is_empty()
        );

        // The constraint now holds for new rows.
        assert!(insert_checkpoint(&database, session_id, 7).await.is_err());
        insert_checkpoint(&database, session_id, 9)
            .await
            .expect("a new key should insert");
    }
}

#[cfg(feature = "mssql")]
#[test]
fn sql_server_run_state_has_an_ordered_candidate_index() {
    let catalog = SchemaModuleCatalog::compose(&[&AiSchemaModule]).unwrap();
    let schema = catalog.schema_model();
    let runs = schema
        .tables
        .iter()
        .find(|table| table.table_name == "[graphql_orm_ai_runs]")
        .unwrap();
    for (timestamp, created_at) in [("lease_expires_at", false), ("next_attempt_at", true)] {
        let mut columns = vec!["state", timestamp, "recovery_examined_at"];
        if created_at {
            columns.push("created_at");
        }
        columns.push("id");
        assert!(runs.indexes.iter().any(|index| index.columns == columns));
    }
    assert!(runs.columns.iter().any(|column| {
        column.name == "state" && column.sql_type == "NVARCHAR(450) COLLATE Latin1_General_100_BIN2"
    }));
}
