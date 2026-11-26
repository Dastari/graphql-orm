use graphql_orm::graphql::orm::{
    ApplyOptions, IndexDef, IndexDirection, IntrospectionBackend, MigrationRisk, MigrationStep,
    OrmBackend, OrmSchemaModule, PlannedMigration, SchemaModel, SqlValue, UniqueIndexDuplicates,
    WriteBackend,
};
use graphql_orm::prelude::Database;
use graphql_orm_ai::{AiOrmBackend, AiSchemaModule};
use uuid::Uuid;

#[cfg(not(feature = "mssql"))]
const TABLE: &str = "graphql_orm_ai_approvals";
#[cfg(feature = "mssql")]
const TABLE: &str = "[graphql_orm_ai_approvals]";
const LEGACY_NAME: &str = "idx_graphql_orm_ai_approvals_tool_call_id";
const KEY: &[&str] = &["tool_call_id"];
#[cfg(not(feature = "mssql"))]
const WEBHOOK_TABLE: &str = "graphql_orm_ai_provider_webhook_receipts";
#[cfg(not(feature = "mssql"))]
const LEGACY_WEBHOOK: &str = "idx_graphql_orm_ai_provider_webhook_receipts_provider_kind";

// Module 0.83.0 replaced the approval lookup index and added two other
// unique keys. The SQL Server fixture covers the approvals table only.
const UNIQUE_KEYS_ADDED_IN_083: &[(&str, &[&str])] = &[
    (TABLE, KEY),
    #[cfg(not(feature = "mssql"))]
    (
        "graphql_orm_ai_run_attempts",
        &["run_id", "lease_generation"],
    ),
    #[cfg(not(feature = "mssql"))]
    (
        "graphql_orm_ai_context_checkpoints",
        &["session_id", "through_sequence"],
    ),
];

fn legacy_module(target: &SchemaModel) -> SchemaModel {
    let mut model = target.clone();
    for &(table_name, columns) in UNIQUE_KEYS_ADDED_IN_083 {
        let table = model
            .tables
            .iter_mut()
            .find(|table| table.table_name == table_name)
            .expect("AI table");
        let index = table
            .indexes
            .iter()
            .position(|index| index.is_unique && index.columns == columns)
            .expect("unique index introduced in 0.83.0");
        table.indexes.remove(index);
        if table_name == TABLE {
            table.indexes.push(IndexDef::new(LEGACY_NAME, KEY));
        }
    }
    #[cfg(not(feature = "mssql"))]
    {
        let webhook = model
            .tables
            .iter_mut()
            .find(|table| table.table_name == WEBHOOK_TABLE)
            .expect("AI webhook table");
        webhook.indexes.retain(|index| {
            index.columns
                != [
                    "provider_kind",
                    "provider_profile_id",
                    "provider_response_id",
                    "state",
                ]
        });
        webhook
            .indexes
            .push(IndexDef::new(LEGACY_WEBHOOK, &["provider_kind"]));
    }
    model
}

async fn live_schema(database: &Database<AiOrmBackend>) -> SchemaModel {
    AiOrmBackend::introspect_schema_including(database.pool(), &[TABLE])
        .await
        .expect("inspect the live schema")
}

async fn apply(database: &Database<AiOrmBackend>, plan: &PlannedMigration, options: ApplyOptions) {
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    database
        .schema()
        .apply_migration(plan, options)
        .await
        .expect("apply schema migration");

    #[cfg(feature = "mssql")]
    {
        assert!(
            options.allow_destructive
                || plan
                    .steps
                    .iter()
                    .all(|step| step.risk != MigrationRisk::Destructive)
        );
        let mut transaction = AiOrmBackend::begin_write_transaction(
            database.pool(),
            graphql_orm::graphql::orm::TransactionMode::default(),
        )
        .await
        .expect("begin external schema migration");
        for statement in &plan.statements {
            AiOrmBackend::execute_in_transaction(&mut transaction, statement, &[])
                .await
                .unwrap_or_else(|error| panic!("execute {statement}: {error}"));
        }
        AiOrmBackend::commit_write_transaction(transaction)
            .await
            .expect("commit external schema migration");
    }
}

async fn insert_approval(
    database: &Database<AiOrmBackend>,
    id: Uuid,
    tool_call: Uuid,
) -> graphql_orm::Result<()> {
    let [id_param, call_param, session_param] = [1, 2, 3].map(AiOrmBackend::placeholder);
    AiOrmBackend::execute_with_binds(
        database.pool(),
        &format!(
            "INSERT INTO {TABLE} (id, tool_call_id, session_id, principal_subject, \
             principal_reference_fingerprint, argument_hash, tool_fingerprint, binding_hash, \
             execution_target_id, target_schema_fingerprint, operation_name, operation_document_hash, \
             result_projection_fingerprint, disclosure_schema_fingerprint, policy_version, \
             authorization_state_digest, action_preview_hash, state, recent_mfa_required, \
             created_at, expires_at, maximum_uses, consumed_uses) VALUES \
             ({id_param}, {call_param}, {session_param}, 'owner', 'principal', 'args', 'tool', 'binding', \
             'local', 'schema', 'operation', 'document', 'projection', 'disclosure', 'policy', \
             'authorization', 'preview', 'pending', {}, 1, 2, 1, 0)",
            if cfg!(feature = "postgres") { "false" } else { "0" }
        ),
        &[SqlValue::Uuid(id), SqlValue::Uuid(tool_call), SqlValue::Uuid(Uuid::new_v4())],
    )
    .await
    .map(|_| ())
}

async fn matching_approvals(database: &Database<AiOrmBackend>, filter: &str, id: Uuid) -> usize {
    AiOrmBackend::fetch_rows(
        database.pool(),
        &format!(
            "SELECT principal_subject FROM {TABLE} WHERE {filter} = {} AND principal_subject = 'owner'",
            AiOrmBackend::placeholder(1)
        ),
        &[SqlValue::Uuid(id)],
    )
    .await
    .expect("read preserved approvals")
    .len()
}

pub(crate) async fn legacy_approval_upgrade(database: &Database<AiOrmBackend>) {
    let schema = database.schema();
    let module = AiSchemaModule;
    let mut entities = module.entities().to_vec();
    // SQL Server's pre-0.83 schema-only profile had the same approval index.
    // Other tables also need independently reviewed SQL Server upgrades.
    if cfg!(feature = "mssql") {
        entities.retain(|entity| entity.table_name == TABLE);
    }
    #[cfg(feature = "mssql")]
    database
        .pool()
        .install_restore_marker()
        .await
        .expect("install SQL Server restore marker");
    let target = SchemaModel::from_entities(&entities);
    let initial = schema
        .plan_migration(
            "0.82.0",
            "legacy AI module",
            &live_schema(database).await,
            &legacy_module(&target),
        )
        .expect("plan legacy AI module");
    apply(database, &initial, ApplyOptions::default()).await;

    // An approvals-only plan still hashes the rest of the live catalog.
    // Ensure admission uses the same SQL Server name normalization as planning.
    #[cfg(feature = "mssql")]
    AiOrmBackend::execute_with_binds(
        database.pool(),
        "CREATE TABLE dbo.graphql_orm_ai_provider_webhook_receipts (id BIGINT NOT NULL PRIMARY KEY)",
        &[],
    )
    .await
    .expect("create an existing AI table outside the approval plan");

    let approval = Uuid::new_v4();
    let tool_call = Uuid::new_v4();
    insert_approval(database, approval, tool_call)
        .await
        .expect("legacy store accepts approval");

    let upgrade = schema
        .plan_migration_to_entities("current-ai", "current AI module", &entities)
        .await
        .expect("plan direct upgrade to current module");
    assert!(upgrade.steps.iter().any(|step| matches!(
        &step.step,
        MigrationStep::DropIndex { table_name, index_name }
            if table_name == TABLE && index_name == LEGACY_NAME
    )));
    #[cfg(not(feature = "mssql"))]
    assert!(upgrade.steps.iter().any(|step| matches!(
        &step.step,
        MigrationStep::DropIndex { table_name, index_name }
            if table_name == WEBHOOK_TABLE && index_name == LEGACY_WEBHOOK
    )));
    let replacement = upgrade
        .steps
        .iter()
        .find_map(|step| match &step.step {
            MigrationStep::CreateIndex { table_name, index }
                if table_name == TABLE && index.is_unique && index.columns == KEY =>
            {
                Some(index.clone())
            }
            _ => None,
        })
        .expect("upgrade creates the matching key");
    let before = live_schema(database).await;

    let mut wrong_table = upgrade.clone();
    let drop = wrong_table
        .steps
        .iter_mut()
        .find_map(|step| match &mut step.step {
            MigrationStep::DropIndex {
                table_name,
                index_name,
            } if index_name == LEGACY_NAME => Some(table_name),
            _ => None,
        })
        .expect("approval lookup drop");
    *drop = "other_table".into();
    let options = module
        .admit_index_replacements_with_live_schema(database, &wrong_table, ApplyOptions::default())
        .await
        .expect("inspect index drop on an unrelated table");
    assert!(!options.allow_destructive);

    let mut altered_target = target.clone();
    let approvals = altered_target
        .tables
        .iter_mut()
        .find(|table| table.table_name == TABLE)
        .unwrap();
    approvals.indexes.retain(|index| index != &replacement);
    let without_create = schema
        .plan_migration("drop-only", "no matching create", &before, &altered_target)
        .expect("plan approval index drop without replacement");
    assert!(without_create.steps.iter().any(|step| matches!(
        &step.step,
        MigrationStep::DropIndex { table_name, index_name }
            if table_name == TABLE && index_name == LEGACY_NAME
    )));
    let options = module
        .admit_index_replacements_with_live_schema(
            database,
            &without_create,
            ApplyOptions::default(),
        )
        .await
        .expect("inspect incomplete plan");
    assert!(!options.allow_destructive);

    let mut altered_target = target.clone();
    let approvals = altered_target
        .tables
        .iter_mut()
        .find(|table| table.table_name == TABLE)
        .unwrap();
    approvals
        .indexes
        .retain(|index| index.columns != ["session_id"]);
    let with_unrelated_drop = schema
        .plan_migration(
            "extra-drop",
            "unrelated index drop",
            &before,
            &altered_target,
        )
        .expect("plan approval replacement with unrelated drop");
    assert_eq!(
        with_unrelated_drop
            .steps
            .iter()
            .filter(|step| step.risk == MigrationRisk::Destructive)
            .count(),
        upgrade
            .steps
            .iter()
            .filter(|step| step.risk == MigrationRisk::Destructive)
            .count()
            + 1,
        "{:?}",
        with_unrelated_drop.steps
    );
    let options = module
        .admit_index_replacements_with_live_schema(
            database,
            &with_unrelated_drop,
            ApplyOptions::default(),
        )
        .await
        .expect("inspect mixed plan");
    assert!(!options.allow_destructive);

    let duplicate = Uuid::new_v4();
    insert_approval(database, duplicate, tool_call)
        .await
        .expect("legacy lookup permits repeated keys");
    let error = module
        .admit_index_replacements_with_live_schema(database, &upgrade, ApplyOptions::default())
        .await
        .expect_err("preflight refuses duplicate before DDL");
    let conflict = UniqueIndexDuplicates::from_error(&error).expect("typed duplicate-key refusal");
    assert_eq!(conflict.table, TABLE);
    assert_eq!(conflict.index, replacement.name);
    assert_eq!(conflict.columns, KEY);
    assert_eq!(
        live_schema(database).await.stable_hash(),
        before.stable_hash()
    );
    assert_eq!(
        matching_approvals(database, "tool_call_id", tool_call).await,
        2
    );

    AiOrmBackend::execute_with_binds(
        database.pool(),
        &format!(
            "DELETE FROM {TABLE} WHERE id = {}",
            AiOrmBackend::placeholder(1)
        ),
        &[SqlValue::Uuid(duplicate)],
    )
    .await
    .expect("resolve duplicate approval");

    for wrong_index in [
        IndexDef::new(LEGACY_NAME, &["session_id"]),
        IndexDef::new(LEGACY_NAME, KEY)
            .with_directions(&[IndexDirection::Desc])
            .unique(),
    ] {
        let mut wrong_schema = before.clone();
        let approvals = wrong_schema
            .tables
            .iter_mut()
            .find(|table| table.table_name == TABLE)
            .unwrap();
        let old_index = approvals
            .indexes
            .iter_mut()
            .find(|index| index.name == LEGACY_NAME)
            .unwrap();
        *old_index = wrong_index;
        let live = live_schema(database).await;
        let drift = schema
            .plan_migration(
                format!("wrong-index-{}", old_index.is_unique),
                "different live index",
                &live,
                &wrong_schema,
            )
            .expect("plan fixture index drift");
        apply(
            database,
            &drift,
            ApplyOptions {
                allow_destructive: true,
                ..ApplyOptions::default()
            },
        )
        .await;
        let upgrade_from_wrong_index = schema
            .plan_migration_to_entities(
                "wrong-source",
                "upgrade with different source index",
                &entities,
            )
            .await
            .expect("plan from a same-named but different index");
        assert!(upgrade_from_wrong_index.steps.iter().any(|step| matches!(
            &step.step,
            MigrationStep::DropIndex { table_name, index_name }
                if table_name == TABLE && index_name == LEGACY_NAME
        )));
        let options = module
            .admit_index_replacements_with_live_schema(
                database,
                &upgrade_from_wrong_index,
                ApplyOptions::default(),
            )
            .await
            .expect("inspect same-named index");
        assert!(!options.allow_destructive);
    }

    let restore_legacy_index = schema
        .plan_migration(
            "restore-legacy-index",
            "restore fixture index",
            &live_schema(database).await,
            &before,
        )
        .expect("plan fixture index restoration");
    apply(
        database,
        &restore_legacy_index,
        ApplyOptions {
            allow_destructive: true,
            ..ApplyOptions::default()
        },
    )
    .await;

    #[cfg(not(feature = "mssql"))]
    let (drop_legacy, index_suffix) = (
        format!("DROP INDEX {LEGACY_NAME}"),
        "WHERE state = 'pending'",
    );
    #[cfg(feature = "mssql")]
    let (drop_legacy, index_suffix) = (
        format!("DROP INDEX {LEGACY_NAME} ON {TABLE}"),
        "INCLUDE (state)",
    );
    for suffix in [index_suffix, ""] {
        AiOrmBackend::execute_with_binds(database.pool(), &drop_legacy, &[])
            .await
            .expect("drop approval index for catalog fixture");
        AiOrmBackend::execute_with_binds(
            database.pool(),
            &format!("CREATE INDEX {LEGACY_NAME} ON {TABLE} (tool_call_id) {suffix}"),
            &[],
        )
        .await
        .expect("create approval index for catalog fixture");
        let plan = schema
            .plan_migration_to_entities("catalog-index", "upgrade catalog fixture", &entities)
            .await
            .expect("plan upgrade for catalog fixture");
        // SQLite and PostgreSQL include the predicate in the schema hash.
        // SQL Server still relies on the catalog check for included columns.
        if suffix.is_empty() || cfg!(feature = "mssql") {
            assert_eq!(plan.source_schema_hash, upgrade.source_schema_hash);
        } else {
            assert_ne!(plan.source_schema_hash, upgrade.source_schema_hash);
        }
        let options = module
            .admit_index_replacements_with_live_schema(database, &plan, ApplyOptions::default())
            .await
            .expect("inspect approval index catalog fixture");
        assert_eq!(options.allow_destructive, suffix.is_empty(), "{suffix}");
    }

    let options = module
        .admit_index_replacements_with_live_schema(database, &upgrade, ApplyOptions::default())
        .await
        .expect("inspect clean upgrade");
    assert!(options.allow_destructive);
    apply(database, &upgrade, options).await;
    let after = live_schema(database).await;
    let approvals = after
        .tables
        .iter()
        .find(|table| table.table_name == TABLE)
        .unwrap();
    assert!(approvals.indexes.contains(&replacement));
    assert!(!approvals.indexes.contains(&IndexDef::new(LEGACY_NAME, KEY)));
    assert_eq!(matching_approvals(database, "id", approval).await, 1);
    assert!(
        insert_approval(database, duplicate, tool_call)
            .await
            .is_err()
    );
    let replan = schema
        .plan_migration_to_entities("current-ai", "verify upgrade", &entities)
        .await
        .expect("replan upgraded module");
    assert!(replan.steps.is_empty(), "{:?}", replan.statements);
}
