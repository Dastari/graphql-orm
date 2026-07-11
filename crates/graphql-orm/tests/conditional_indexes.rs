#![cfg(any(feature = "sqlite", feature = "postgres"))]

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
    let url = std::env::var("TEST_DATABASE_URL")
        .map_err(|error| graphql_orm::Error::Configuration(error.to_string().into()))?;
    let database = Database::<PostgresBackend>::connect_postgres(url).await?;
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
async fn conditional_unique_index_is_idempotent_and_detects_predicate_tampering()
-> graphql_orm::Result<()> {
    if cfg!(feature = "postgres") && std::env::var("TEST_DATABASE_URL").is_err() {
        return Ok(());
    }
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
