// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(any(feature = "sqlite", feature = "postgres"))]

#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use graphql_orm::prelude::*;

#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    table = "conditional_index_records",
    plural = "ConditionalIndexRecords"
)]
#[graphql_orm(conditional_index(
    name = "uidx_conditional_digest_active",
    columns = ["digest"],
    unique = true,
    predicate_field = "status",
    predicate_values = ["APPROVED", "PENDING"]
))]
#[allow(dead_code)]
struct ConditionalIndexRecord {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[graphql_orm(min_length = 32, max_length = 32)]
    digest: Vec<u8>,
    status: String,
    lower: Option<i64>,
    #[graphql_orm(gt_field = "lower")]
    strict_upper: Option<i64>,
    #[graphql_orm(gte_field = "lower")]
    inclusive_upper: Option<i64>,
    #[graphql_orm(lt_field = "strict_upper")]
    strict_lower: Option<i64>,
    #[graphql_orm(lte_field = "inclusive_upper")]
    inclusive_lower: Option<i64>,
}

#[test]
fn conditional_index_metadata_hash_and_all_drift_dimensions_are_structural() {
    let target = SchemaModel::from_entities(&[ConditionalIndexRecord::metadata()]);
    let target_hash = target.stable_hash();
    let mut variants = Vec::new();

    let mut missing = target.clone();
    missing.tables[0].indexes.clear();
    variants.push(missing);

    let mut non_unique = target.clone();
    non_unique.tables[0].indexes[0].is_unique = false;
    variants.push(non_unique);

    let mut wrong_column = target.clone();
    wrong_column.tables[0].indexes[0].columns = &["status"];
    variants.push(wrong_column);

    for values in [&["PENDING"][..], &["APPROVED", "PENDING", "REVOKED"][..]] {
        let mut wrong_predicate = target.clone();
        wrong_predicate.tables[0].indexes[0]
            .predicate
            .as_mut()
            .expect("conditional predicate")
            .values = values;
        variants.push(wrong_predicate);
    }

    for current in variants {
        assert_ne!(current.stable_hash(), target_hash);
        let plan = build_migration_plan(
            #[cfg(feature = "sqlite")]
            DatabaseBackend::Sqlite,
            #[cfg(feature = "postgres")]
            DatabaseBackend::Postgres,
            &current,
            &target,
        );
        assert!(
            plan.steps
                .iter()
                .any(|step| matches!(step, MigrationStep::CreateIndex { .. }))
        );
    }
}

#[cfg(feature = "sqlite")]
async fn database() -> graphql_orm::Result<Database<SqliteBackend>> {
    Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await
}

#[cfg(feature = "postgres")]
async fn database() -> graphql_orm::Result<Database<PostgresBackend>> {
    let pool = owned_postgres::OwnedPostgres::pool("conditional-indexes", 5)
        .await
        .map_err(|error| graphql_orm::Error::Configuration(error.to_string().into()))?;
    let database = Database::<PostgresBackend>::new(pool);
    graphql_orm::sqlx::query("DROP TABLE IF EXISTS conditional_index_records CASCADE")
        .execute(database.pool())
        .await?;
    Ok(database)
}

#[cfg(feature = "sqlite")]
async fn introspect(database: &Database<SqliteBackend>) -> graphql_orm::Result<SchemaModel> {
    introspect_sqlite_schema(database).await
}

#[cfg(feature = "postgres")]
async fn introspect(database: &Database<PostgresBackend>) -> graphql_orm::Result<SchemaModel> {
    introspect_postgres_schema(database).await
}

#[cfg(feature = "sqlite")]
const VALID_INSERT: &str = "INSERT INTO conditional_index_records
    (id, digest, status, lower, strict_upper, inclusive_upper, strict_lower, inclusive_lower)
    VALUES ('00000000-0000-0000-0000-000000000001',
    X'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
    'PENDING', 1, 2, 1, 1, 1)";
#[cfg(feature = "postgres")]
const VALID_INSERT: &str = "INSERT INTO conditional_index_records
    (id, digest, status, lower, strict_upper, inclusive_upper, strict_lower, inclusive_lower)
    VALUES ('00000000-0000-0000-0000-000000000001',
    decode('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'hex'),
    'PENDING', 1, 2, 1, 1, 1)";

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn conditional_unique_index_is_idempotent_and_detects_predicate_tampering()
-> graphql_orm::Result<()> {
    let database = database().await?;
    let entities = [ConditionalIndexRecord::metadata()];
    let target = SchemaModel::from_entities(&entities);
    let version = format!("conditional-index-{}", graphql_orm::uuid::Uuid::new_v4());
    let plan = database
        .schema()
        .plan_migration_to_entities_with_options(
            &version,
            "conditional index",
            &entities,
            PlanOptions::managed_tables_only(),
        )
        .await?;
    assert!(plan.statements.iter().any(|statement| {
        statement.contains("UNIQUE INDEX")
            && statement.contains("WHERE")
            && statement.contains("'APPROVED', 'PENDING'")
    }));
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    graphql_orm::sqlx::query(VALID_INSERT)
        .execute(database.pool())
        .await?;

    #[cfg(feature = "sqlite")]
    let archived = VALID_INSERT
        .replace("000000000001", "000000000002")
        .replace("'PENDING'", "'ARCHIVED'");
    #[cfg(feature = "postgres")]
    let archived = VALID_INSERT
        .replace("000000000001", "000000000002")
        .replace("'PENDING'", "'ARCHIVED'");
    graphql_orm::sqlx::query(&archived)
        .execute(database.pool())
        .await?;
    let duplicate = VALID_INSERT.replace("000000000001", "000000000003");
    let error = graphql_orm::sqlx::query(&duplicate)
        .execute(database.pool())
        .await
        .expect_err("active digest uniqueness is enforced");
    assert_eq!(
        OrmPublicError::from_sqlx(&error).code,
        OrmErrorCode::ConstraintViolation
    );

    // SQL comparisons evaluate UNKNOWN for NULL, so nullable pairs pass.
    let nullable = VALID_INSERT
        .replace("000000000001", "000000000004")
        .replace(
            "'PENDING', 1, 2, 1, 1, 1",
            "'ARCHIVED', NULL, NULL, NULL, NULL, NULL",
        );
    graphql_orm::sqlx::query(&nullable)
        .execute(database.pool())
        .await?;
    let invalid_comparison = VALID_INSERT
        .replace("000000000001", "000000000005")
        .replace("'PENDING', 1, 2, 1, 1, 1", "'ARCHIVED', 2, 2, 2, 2, 2");
    assert!(
        graphql_orm::sqlx::query(&invalid_comparison)
            .execute(database.pool())
            .await
            .is_err()
    );

    let live = introspect(&database).await?;
    let clean = database
        .schema()
        .plan_migration("conditional-clean", "clean", &live, &target)?;
    assert!(
        clean.steps.iter().all(|step| !matches!(
            step.step,
            MigrationStep::DropIndex { .. } | MigrationStep::CreateIndex { .. }
        )),
        "conditional index must be restart-idempotent: {:?}",
        clean.steps
    );

    graphql_orm::sqlx::query("DROP INDEX uidx_conditional_digest_active")
        .execute(database.pool())
        .await?;
    graphql_orm::sqlx::query(
        "CREATE UNIQUE INDEX uidx_conditional_digest_active
         ON conditional_index_records (\"digest\")
         WHERE ((\"status\" IN ('PENDING', 'APPROVED', 'PENDING')))",
    )
    .execute(database.pool())
    .await?;
    let harmless_live = introspect(&database).await?;
    let harmless = database.schema().plan_migration(
        "conditional-harmless-canonicalization",
        "quoted, parenthesized, reordered and deduplicated",
        &harmless_live,
        &target,
    )?;
    assert!(
        harmless.steps.iter().all(|step| !matches!(
            step.step,
            MigrationStep::DropIndex { .. } | MigrationStep::CreateIndex { .. }
        )),
        "documented harmless predicate differences remain equivalent: {:?}",
        harmless.steps
    );

    graphql_orm::sqlx::query("DELETE FROM conditional_index_records WHERE status <> 'PENDING'")
        .execute(database.pool())
        .await?;
    #[cfg(feature = "sqlite")]
    let tampered_predicates = [
        "status IN ('PENDING')",
        "status IN ('APPROVED', 'PENDING') OR status IS NOT NULL",
        "status IN ('APPROVED', 'PENDING') AND status IS NOT NULL",
        "status IN ('APPROVED', 'PENDING') /* trailing managed-looking comment */",
    ];
    // PostgreSQL deliberately removes comments while storing an index expression,
    // so a comment-only spelling cannot be distinguished through pg_get_expr.
    #[cfg(feature = "postgres")]
    let tampered_predicates = [
        "status IN ('PENDING')",
        "status IN ('APPROVED', 'PENDING') OR status IS NOT NULL",
        "status IN ('APPROVED', 'PENDING') AND status IS NOT NULL",
        "lower(status) IN ('approved', 'pending')",
    ];
    let mut last_repair = None;
    for predicate in tampered_predicates {
        graphql_orm::sqlx::query("DROP INDEX uidx_conditional_digest_active")
            .execute(database.pool())
            .await?;
        graphql_orm::sqlx::query(&format!(
            "CREATE UNIQUE INDEX uidx_conditional_digest_active
             ON conditional_index_records (digest) WHERE {predicate}"
        ))
        .execute(database.pool())
        .await?;

        let tampered = introspect(&database).await?;
        let repair = database.schema().plan_migration(
            &version,
            "recorded version must fail",
            &tampered,
            &target,
        )?;
        assert!(
            repair.steps.iter().any(|step| matches!(
                step.step,
                MigrationStep::DropIndex { .. } | MigrationStep::CreateIndex { .. }
            )),
            "unsupported or partial predicate must be drift: {predicate}"
        );
        last_repair = Some(repair);
    }
    let repair = last_repair.expect("at least one tampered predicate");
    database
        .schema()
        .apply_migration(&repair, ApplyOptions::default())
        .await
        .expect_err("recorded migration with conditional-index drift fails closed");

    #[cfg(feature = "postgres")]
    graphql_orm::sqlx::query("DROP TABLE IF EXISTS conditional_index_records CASCADE")
        .execute(database.pool())
        .await?;
    Ok(())
}

#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    table = "conditional_index_records",
    plural = "ConditionalPreflightRecords"
)]
#[graphql_orm(conditional_index(
    name = "uidx_preflight_digest",
    columns = ["digest"],
    unique = true,
    predicate_field = "status",
    predicate_values = ["status"]
))]
#[allow(dead_code)]
struct ConditionalPreflightRecord {
    #[primary_key]
    id: String,
    digest: String,
    status: Option<String>,
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn conditional_unique_preflight_respects_new_and_existing_predicate_columns()
-> graphql_orm::Result<()> {
    for (add_status, included, refused) in [
        (true, false, false),
        (false, true, true),
        (false, false, false),
    ] {
        let database = database().await?;
        let mut before = ConditionalPreflightRecord::metadata().clone();
        before.indexes = Box::new([]);
        if add_status {
            before.fields = before
                .fields
                .iter()
                .filter(|c| c.name != "status")
                .cloned()
                .collect::<Vec<_>>()
                .into_boxed_slice();
        }
        let plan = database
            .schema()
            .plan_migration_to_entities(
                "preflight-v1",
                "before conditional index",
                &[Box::leak(Box::new(before))],
            )
            .await?;
        database
            .schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await?;
        let sql = if add_status {
            "INSERT INTO conditional_index_records (id, digest) VALUES ('1', 'same'), ('2', 'same')"
        } else if included {
            "INSERT INTO conditional_index_records (id, digest, status) VALUES ('1', 'same', 'status'), ('2', 'same', 'status')"
        } else {
            "INSERT INTO conditional_index_records (id, digest, status) VALUES ('1', 'same', 'outside'), ('2', 'same', 'outside'), ('3', 'same', 'status')"
        };
        graphql_orm::sqlx::query(sql)
            .execute(database.pool())
            .await?;
        let plan = database
            .schema()
            .plan_migration_to_entities(
                "preflight-v2",
                "conditional index",
                &[ConditionalPreflightRecord::metadata()],
            )
            .await?;
        let result = database
            .schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await;
        if refused {
            let error = result.expect_err("duplicates inside the predicate must be refused");
            assert!(
                graphql_orm::graphql::orm::UniqueIndexDuplicates::from_error(&error).is_some(),
                "{error:?}"
            );
            assert_eq!(
                database.schema().current_version().await?,
                Some("preflight-v1".into())
            );
        } else {
            result?;
            assert_eq!(
                database.schema().current_version().await?,
                Some("preflight-v2".into())
            );
        }
    }
    Ok(())
}

#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(table = "dotted_index_records", plural = "DottedIndexRecords")]
#[graphql_orm(conditional_index(
    name = "idx_dotted_active",
    columns = ["id"],
    predicate_field = "status",
    predicate_values = ["ACTIVE", "PENDING"]
))]
#[allow(dead_code)]
struct DottedIndexRecord {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[graphql_orm(db_column = "\"state.code\"")]
    status: String,
    #[graphql_orm(db_column = "status")]
    plain_status: String,
    #[graphql_orm(db_column = "\"\"\"status\"\"\"")]
    quoted_status: String,
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn conditional_index_with_literal_dotted_predicate_has_no_planned_changes()
-> graphql_orm::Result<()> {
    let database = database().await?;
    let entities = [DottedIndexRecord::metadata()];
    let plan = database
        .schema()
        .plan_migration_to_entities_with_options(
            "dotted-condition",
            "literal dotted predicate",
            &entities,
            PlanOptions::managed_tables_only(),
        )
        .await?;
    assert!(
        plan.statements
            .iter()
            .any(|statement| statement.contains("WHERE \"state.code\" IN"))
    );
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;

    let clean = database
        .schema()
        .plan_migration_to_entities_with_options(
            "dotted-condition-clean",
            "replan literal dotted predicate",
            &entities,
            PlanOptions::managed_tables_only(),
        )
        .await?;
    assert!(clean.steps.is_empty(), "{:?}", clean.steps);
    // Hand-built metadata can supply the same physical column without delimiters.
    let mut target = SchemaModel::from_entities(&entities);
    target.tables[0].indexes[0]
        .predicate
        .as_mut()
        .unwrap()
        .column = "state.code";
    let mut live = introspect(&database).await?;
    live.tables
        .retain(|table| table.table_name == "dotted_index_records");
    let clean = database.schema().plan_migration(
        "dotted-condition-raw-name",
        "replan using a raw predicate column name",
        &live,
        &target,
    )?;
    assert!(clean.steps.is_empty(), "{:?}", clean.steps);
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn conditional_index_detects_switch_from_quoted_name_to_plain_name() -> graphql_orm::Result<()>
{
    let database = database().await?;
    let mut quoted = SchemaModel::from_entities(&[DottedIndexRecord::metadata()]);
    quoted.tables[0].table_name = "quoted_index_records".to_owned();
    quoted.tables[0].indexes[0].name = "idx_quoted_active";
    let predicate = quoted.tables[0].indexes[0].predicate.as_mut().unwrap();
    predicate.column = "\"\"\"status\"\"\"";
    let only_test_table = |mut schema: SchemaModel| {
        schema
            .tables
            .retain(|table| table.table_name == "quoted_index_records");
        schema
    };
    let current = only_test_table(introspect(&database).await?);
    let plan = database.schema().plan_migration(
        "quoted-name",
        "index on the column whose name includes quotes",
        &current,
        &quoted,
    )?;
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;

    let live = only_test_table(introspect(&database).await?);
    let clean = database.schema().plan_migration(
        "quoted-name-clean",
        "replan the quoted-name index",
        &live,
        &quoted,
    )?;
    assert!(clean.steps.is_empty(), "{:?}", clean.steps);

    let mut plain = quoted;
    plain.tables[0].indexes[0]
        .predicate
        .as_mut()
        .unwrap()
        .column = "status";
    let change = database.schema().plan_migration(
        "quoted-name-to-plain",
        "switch predicate to the plain column",
        &live,
        &plain,
    )?;
    assert!(
        matches!(
            change.steps.as_slice(),
            [drop, create]
                if matches!(drop.step, MigrationStep::DropIndex { .. })
                    && matches!(create.step, MigrationStep::CreateIndex { .. })
        ),
        "{:?}",
        change.steps
    );
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn opaque_partial_index_differs_from_plain_index() -> graphql_orm::Result<()> {
    use graphql_orm::graphql::orm::{
        SchemaDiagnosticKind, render_migration_step, try_render_migration_step,
    };

    let database = database().await?;
    graphql_orm::sqlx::query(
        "CREATE TABLE conditional_index_records (id UUID PRIMARY KEY, digest BYTEA, status TEXT)",
    )
    .execute(database.pool())
    .await?;
    graphql_orm::sqlx::query(
        "CREATE UNIQUE INDEX uidx_conditional_digest_active
         ON conditional_index_records (digest) WHERE/* comment */lower(status) = 'pending'",
    )
    .execute(database.pool())
    .await?;

    let live = introspect(&database).await?;
    let live_index = &live.tables[0].indexes[0];
    let index_name = live_index.name;
    let mut target = live.clone();
    target.tables[0].indexes[0].predicate = None;
    let predicate = live_index.predicate.as_ref().expect("partial index filter");
    assert!(predicate.column.is_empty());
    assert!(predicate.values[0].contains("pending"));
    assert_ne!(live_index, &target.tables[0].indexes[0]);
    assert_ne!(live.stable_hash(), target.stable_hash());

    let manager = database.schema();
    assert!(
        manager
            .validate(&live, &target)?
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.kind == SchemaDiagnosticKind::IndexMismatch)
    );
    let plan = manager.plan_migration("plain-index", "plain index", &live, &target)?;
    assert!(plan.steps.iter().any(|step| matches!(
        &step.step,
        MigrationStep::DropIndex { index_name: name, .. } if name == index_name
    )));
    assert!(plan.steps.iter().any(|step| matches!(
        &step.step,
        MigrationStep::CreateIndex { index, .. }
            if index.name == index_name && index.predicate.is_none()
    )));
    let error = manager
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect_err("the replacement must pass the existing destructive gate");
    assert!(error.to_string().contains("allow_destructive"));

    let recreate_opaque = MigrationStep::CreateIndex {
        table_name: live.tables[0].table_name.clone(),
        index: live_index.clone(),
    };
    #[cfg(feature = "sqlite")]
    let backend = DatabaseBackend::Sqlite;
    #[cfg(feature = "postgres")]
    let backend = DatabaseBackend::Postgres;
    assert!(try_render_migration_step(backend, &recreate_opaque).is_err());

    let recreate_plan = build_migration_plan(backend, &target, &live);
    assert_eq!(recreate_plan.steps.len(), 2);
    let refusal = render_migration_step(backend, &recreate_opaque);
    assert_eq!(refusal.len(), 1);
    assert_eq!(recreate_plan.statements, refusal);
    for statement in [&refusal[0], &recreate_plan.statements[0]] {
        let error = graphql_orm::sqlx::query(statement)
            .execute(database.pool())
            .await
            .expect_err("recreating an opaque predicate must fail at execution");
        #[cfg(feature = "postgres")]
        assert_eq!(
            error.as_database_error().unwrap().message(),
            "opaque index predicate cannot be recreated"
        );
        #[cfg(feature = "sqlite")]
        assert!(error.as_database_error().is_some(), "{error}");
    }

    manager
        .apply_migration(
            &plan,
            ApplyOptions {
                allow_destructive: true,
                ..ApplyOptions::default()
            },
        )
        .await?;
    let repaired = introspect(&database).await?;
    let second = manager.plan_migration("plain-index-v2", "replan", &repaired, &target)?;
    assert!(second.steps.is_empty(), "{:?}", second.steps);

    #[cfg(feature = "postgres")]
    {
        // The predicate has the usual closed-set spelling, but only indexes
        // using the default catalog operator class can decode it as such.
        graphql_orm::sqlx::query(
            "CREATE INDEX idx_nondefault_operator_class
             ON conditional_index_records (status text_pattern_ops) WHERE status IN ('A', 'B')",
        )
        .execute(database.pool())
        .await?;
        let live = introspect(&database).await?;
        let predicate = live.tables[0]
            .indexes
            .iter()
            .find(|index| index.name == "idx_nondefault_operator_class")
            .and_then(|index| index.predicate.as_ref())
            .expect("filtered index with nondefault operator class");
        assert!(predicate.column.is_empty());
        assert!(predicate.values[0].contains("ANY"));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn postgres_closed_set_predicates_round_trip() -> graphql_orm::Result<()> {
    let database = database().await?;
    let manager = database.schema();
    let mut target = SchemaModel::from_entities(&[ConditionalIndexRecord::metadata()]);
    let single = SchemaModel::from_entities(&[ConditionalPreflightRecord::metadata()]);
    target.tables[0]
        .indexes
        .push(single.tables[0].indexes[0].clone());

    let before = introspect(&database).await?;
    let plan = manager.plan_migration(
        "closed-sets",
        "create conditional indexes",
        &before,
        &target,
    )?;
    manager
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    let live = introspect(&database).await?;
    for declared in &target.tables[0].indexes {
        let actual = live.tables[0]
            .indexes
            .iter()
            .find(|index| index.name == declared.name)
            .expect("created index");
        assert_eq!(actual.predicate, declared.predicate, "{}", declared.name);
    }
    let second = manager.plan_migration("closed-sets-v2", "replan", &live, &target)?;
    assert!(second.steps.is_empty(), "{:?}", second.steps);
    Ok(())
}
