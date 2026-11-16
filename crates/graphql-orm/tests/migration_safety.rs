//! Data-safety defaults of migration planning and application:
//!
//! - a plan drops a live table only when the caller plans with
//!   `PlanOptions::strict()`; the default leaves undeclared tables alone;
//! - a column type change that can lose stored values, and every step that
//!   removes an integrity guarantee, is `Destructive` and needs
//!   `ApplyOptions::allow_destructive`.
#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
use graphql_orm::graphql::orm::*;
use graphql_orm::prelude::*;

#[cfg(feature = "mssql")]
#[allow(dead_code)]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    repository_entity(
        backend = "sqlite",
        table = "safety_records",
        schema_policy = "managed",
        unique_composite = "owner_id,name"
    )
)]
#[cfg_attr(
    feature = "postgres",
    repository_entity(
        backend = "postgres",
        table = "safety_records",
        schema_policy = "managed",
        unique_composite = "owner_id,name"
    )
)]
#[cfg_attr(
    feature = "mssql",
    repository_entity(
        backend = "mssql",
        table = "dbo.safety_records",
        schema_policy = "managed",
        unique_composite = "owner_id,name"
    )
)]
struct SafetyRecord {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: String,
    owner_id: String,
    name: String,
    amount: f64,
}

fn column(sql_type: &str) -> ColumnModel {
    ColumnModel {
        name: "amount".into(),
        sql_type: sql_type.into(),
        spatial: None,
        nullable: false,
        is_primary_key: false,
        is_unique: false,
        default: None,
    }
}

fn retype(before: &str, after: &str) -> PlannedMigrationStep {
    classify_migration_step(&MigrationStep::AlterColumn {
        table_name: "safety_records".into(),
        before: column(before),
        after: column(after),
    })
}

/// A type change is `Risky` only when it is a known lossless widening. Every
/// other change, in the spelling of any backend, needs the destructive gate.
#[test]
fn lossy_type_changes_are_destructive_and_lossless_widenings_are_not() {
    for (before, after) in [
        // PostgreSQL: the conversions named in the report, and other narrowings.
        ("DOUBLE PRECISION", "INTEGER"),
        ("NUMERIC(12,4)", "NUMERIC(12,2)"),
        ("NUMERIC(12,2)", "NUMERIC(10,2)"),
        ("TIMESTAMPTZ", "DATE"),
        ("timestamp with time zone", "date"),
        ("BIGINT", "INTEGER"),
        ("bigint", "double precision"),
        ("character varying(50)", "VARCHAR(20)"),
        ("TEXT", "VARCHAR(20)"),
        ("TEXT", "INTEGER"),
        ("JSON", "JSONB"),
        ("INTEGER[]", "BIGINT[]"),
        // SQL Server.
        ("FLOAT", "INT"),
        ("NVARCHAR(100)", "NVARCHAR(50)"),
        ("NVARCHAR(MAX)", "NVARCHAR(50)"),
        ("DATETIME2", "DATE"),
        ("DECIMAL(12,4)", "DECIMAL(12,2)"),
        ("BIGINT", "TINYINT"),
        // SQLite.
        ("REAL", "INTEGER"),
        ("INTEGER", "REAL"),
        ("TEXT", "BLOB"),
        // A type the classifier does not know is never assumed safe.
        ("citext", "ltree"),
    ] {
        let step = retype(before, after);
        assert_eq!(
            step.risk,
            MigrationRisk::Destructive,
            "{before} -> {after}: {}",
            step.reason
        );
    }
    for (before, after) in [
        ("SMALLINT", "INTEGER"),
        ("integer", "BIGINT"),
        ("INTEGER", "NUMERIC(12,2)"),
        ("INTEGER", "NUMERIC"),
        ("INTEGER", "DOUBLE PRECISION"),
        ("REAL", "DOUBLE PRECISION"),
        ("NUMERIC(10,2)", "NUMERIC(12,2)"),
        ("NUMERIC(10,2)", "NUMERIC(14,4)"),
        ("NUMERIC(10,2)", "NUMERIC"),
        ("character varying(20)", "VARCHAR(50)"),
        ("VARCHAR(20)", "TEXT"),
        ("INT", "BIGINT"),
        ("TINYINT", "INT"),
        ("NVARCHAR(50)", "NVARCHAR(100)"),
        ("NVARCHAR(50)", "NVARCHAR(MAX)"),
        ("DECIMAL(10,2)", "DECIMAL(12,2)"),
    ] {
        let step = retype(before, after);
        assert_eq!(
            step.risk,
            MigrationRisk::Risky,
            "{before} -> {after}: {}",
            step.reason
        );
    }
    // An integer does not fit a numeric with too few integer digits.
    assert_eq!(
        retype("BIGINT", "NUMERIC(12,2)").risk,
        MigrationRisk::Destructive
    );
    // Another spelling of the same type is not a type change.
    assert_eq!(
        retype("character varying(20)", "VARCHAR(20)").risk,
        MigrationRisk::Compatible
    );
    // A lossy change outranks the nullability change made by the same step.
    let mut before = column("DOUBLE PRECISION");
    before.nullable = true;
    let step = classify_migration_step(&MigrationStep::AlterColumn {
        table_name: "safety_records".into(),
        before,
        after: column("INTEGER"),
    });
    assert_eq!(step.risk, MigrationRisk::Destructive);
}

/// Steps that remove an integrity guarantee need the destructive gate; steps
/// that only add one do not.
#[test]
fn integrity_removing_steps_are_destructive() {
    let table_name = || "safety_records".to_string();
    let check = |name: &str, expression: &str| CheckConstraintModel {
        name: name.into(),
        expression: expression.into(),
    };
    let unique = |columns: &[&str]| columns.iter().map(|c| c.to_string()).collect::<Vec<_>>();
    let foreign_key = ForeignKeyModel {
        constraint_name: None,
        column_pairs: vec![ForeignKeyColumnPairModel {
            source_column: "owner_id".into(),
            target_column: "id".into(),
        }],
        target_table: "owners".into(),
        is_multiple: false,
        on_delete: DeletePolicy::Restrict,
    };
    let destructive = [
        MigrationStep::SetAppendOnly {
            table_name: table_name(),
            enabled: false,
            retention_purge: false,
        },
        MigrationStep::DropIndex {
            table_name: table_name(),
            index_name: "safety_records_owner_name".into(),
        },
        MigrationStep::DropForeignKey {
            table_name: table_name(),
            foreign_key: foreign_key.clone(),
        },
        MigrationStep::SetCompositeUniques {
            table_name: table_name(),
            before: vec![unique(&["owner_id", "name"])],
            after: vec![],
        },
        MigrationStep::SetCompositeUniques {
            table_name: table_name(),
            before: vec![unique(&["owner_id", "name"])],
            after: vec![unique(&["owner_id", "id"])],
        },
        MigrationStep::SetCheckConstraints {
            table_name: table_name(),
            before: vec![check("amount_positive", "amount > 0")],
            after: vec![],
        },
        MigrationStep::SetCheckConstraints {
            table_name: table_name(),
            before: vec![check("amount_positive", "amount > 0")],
            after: vec![check("amount_positive", "amount >= 0")],
        },
    ];
    for step in &destructive {
        let planned = classify_migration_step(step);
        assert_eq!(planned.risk, MigrationRisk::Destructive, "{step:?}");
    }
    let additive_or_risky = [
        (
            MigrationStep::SetAppendOnly {
                table_name: table_name(),
                enabled: true,
                retention_purge: false,
            },
            MigrationRisk::Additive,
        ),
        (
            MigrationStep::AddForeignKey {
                table_name: table_name(),
                foreign_key,
            },
            MigrationRisk::Risky,
        ),
        (
            MigrationStep::SetCompositeUniques {
                table_name: table_name(),
                before: vec![unique(&["owner_id", "name"])],
                after: vec![unique(&["owner_id", "name"]), unique(&["name", "id"])],
            },
            MigrationRisk::Risky,
        ),
        (
            MigrationStep::SetCheckConstraints {
                table_name: table_name(),
                before: vec![],
                after: vec![check("amount_positive", "amount > 0")],
            },
            MigrationRisk::Risky,
        ),
    ];
    for (step, risk) in &additive_or_risky {
        assert_eq!(classify_migration_step(step).risk, *risk, "{step:?}");
    }
}

/// `PlanOptions::default()` is managed-tables-only, on every backend.
#[test]
fn default_plan_options_ignore_undeclared_tables() {
    assert_eq!(PlanOptions::default(), PlanOptions::managed_tables_only());
    assert!(PlanOptions::default().ignore_unmanaged_tables);
    assert!(!PlanOptions::strict().ignore_unmanaged_tables);
}

fn drops(plan: &PlannedMigration) -> Vec<String> {
    plan.steps
        .iter()
        .filter_map(|step| match &step.step {
            MigrationStep::DropTable { table_name } => Some(table_name.to_ascii_lowercase()),
            _ => None,
        })
        .collect()
}

/// A populated table the target does not declare survives a default plan
/// applied with `allow_destructive`. Planning with `strict()` is the explicit
/// opt-in that drops undeclared tables, including a table the ORM created
/// whose entity was removed.
#[cfg(any(feature = "sqlite", feature = "postgres"))]
#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn undeclared_tables_survive_unless_the_plan_is_strict()
-> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "sqlite")]
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    #[cfg(feature = "postgres")]
    let mut server = owned_postgres::OwnedPostgres::start("migration-safety-drop")?;
    #[cfg(feature = "postgres")]
    let db = Database::<PostgresBackend>::connect_postgres(&server.url).await?;
    let manager = db.schema();
    let destructive = || ApplyOptions {
        allow_destructive: true,
        ..ApplyOptions::default()
    };

    for sql in [
        "CREATE TABLE host_ledger (id TEXT PRIMARY KEY, total TEXT NOT NULL)",
        "INSERT INTO host_ledger VALUES ('host-row', '42')",
    ] {
        sqlx::query(sql).execute(db.pool()).await?;
    }
    let ledger_rows = || async {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM host_ledger")
            .fetch_one(db.pool())
            .await
    };

    // The default plan creates the entity's table and leaves the host's alone.
    let initial = manager
        .plan_migration_to_entities("v1", "initial", &[SafetyRecord::metadata()])
        .await?;
    assert!(drops(&initial).is_empty(), "{:?}", initial.steps);
    manager.apply_migration(&initial, destructive()).await?;
    assert_eq!(ledger_rows().await?, 1);

    // The entity is removed from the metadata. The default plan drops nothing,
    // neither the host's table nor the table the ORM created.
    let removed = manager
        .plan_migration_to_entities("v2", "entity removed", &[])
        .await?;
    assert!(drops(&removed).is_empty(), "{:?}", removed.steps);
    manager.apply_migration(&removed, destructive()).await?;
    assert_eq!(ledger_rows().await?, 1);

    // `strict()` plans against every live table. Its drops are destructive.
    let strict = manager
        .plan_migration_to_entities_with_options("v3", "strict removal", &[], PlanOptions::strict())
        .await?;
    let mut dropped = drops(&strict);
    dropped.sort();
    assert_eq!(dropped, ["host_ledger", "safety_records"]);
    assert!(
        strict
            .steps
            .iter()
            .all(|step| step.risk == MigrationRisk::Destructive)
    );
    manager
        .apply_migration(&strict, ApplyOptions::default())
        .await
        .expect_err("drops are refused without allow_destructive");
    assert_eq!(ledger_rows().await?, 1);

    // To drop only the ORM's own table in a shared database, plan from a
    // current model that names it and nothing else.
    let live = SchemaModel::from_entities(&[SafetyRecord::metadata()]);
    let empty = SchemaModel {
        extensions: Vec::new(),
        tables: Vec::new(),
    };
    let targeted = manager.plan_migration_with_options(
        "v4",
        "drop owned",
        &live,
        &empty,
        PlanOptions::strict(),
    )?;
    assert_eq!(drops(&targeted), ["safety_records"]);
    manager
        .apply_migration(
            &targeted,
            ApplyOptions {
                allow_destructive: true,
                require_clean_schema: false,
                ..ApplyOptions::default()
            },
        )
        .await?;
    assert_eq!(ledger_rows().await?, 1, "the host table is still there");
    assert!(
        sqlx::query("SELECT 1 FROM safety_records")
            .fetch_optional(db.pool())
            .await
            .is_err(),
        "the ORM table is gone"
    );

    #[cfg(feature = "postgres")]
    server.cleanup()?;
    Ok(())
}

/// SQL Server cannot apply migrations, but it plans them: the default plan
/// has no drop for an undeclared table and a strict plan has.
#[cfg(feature = "mssql")]
#[tokio::test]
#[ignore = "starts a test-owned SQL Server 2022 container"]
async fn sql_server_plans_drop_undeclared_tables_only_when_strict()
-> Result<(), Box<dyn std::error::Error>> {
    let mut server = owned_mssql::OwnedSqlServer::start().await?;
    owned_mssql::execute_batch(
        &server.connection_string,
        "CREATE TABLE dbo.host_ledger (id NVARCHAR(100) NOT NULL PRIMARY KEY, total NVARCHAR(100) NOT NULL); \
         INSERT INTO dbo.host_ledger VALUES (N'host-row', N'42')",
    )
    .await?;
    let pool = graphql_orm::db::mssql::MssqlPool::connect_ado(&server.connection_string).await?;
    let db = Database::builder(pool)
        .schema_policy(SchemaPolicy::PlanOnly)
        .build();
    let manager = db.schema();

    let default = manager
        .plan_migration_to_entities("v1", "initial", &[SafetyRecord::metadata()])
        .await?;
    assert!(drops(&default).is_empty(), "{:?}", default.steps);
    let strict = manager
        .plan_migration_to_entities_with_options(
            "v1",
            "strict",
            &[SafetyRecord::metadata()],
            PlanOptions::strict(),
        )
        .await?;
    let dropped = drops(&strict);
    assert!(
        dropped.iter().any(|table| table.contains("host_ledger")),
        "{dropped:?}"
    );
    server.cleanup()?;
    Ok(())
}

/// Under `ApplyOptions::default()` a plan that removes a uniqueness guarantee
/// or narrows a column is refused and changes nothing; with
/// `allow_destructive` it applies.
#[cfg(any(feature = "sqlite", feature = "postgres"))]
#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn default_apply_refuses_integrity_removing_and_lossy_plans()
-> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "sqlite")]
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    #[cfg(feature = "postgres")]
    let mut server = owned_postgres::OwnedPostgres::start("migration-safety-apply")?;
    #[cfg(feature = "postgres")]
    let db = Database::<PostgresBackend>::connect_postgres(&server.url).await?;
    let manager = db.schema();
    let initial = manager
        .plan_migration_to_entities("v1", "initial", &[SafetyRecord::metadata()])
        .await?;
    manager
        .apply_migration(&initial, ApplyOptions::default())
        .await?;
    sqlx::query(
        "INSERT INTO safety_records (id, owner_id, name, amount) VALUES ('one', 'owner', 'name', 1.9)",
    )
    .execute(db.pool())
    .await?;
    let duplicate = "INSERT INTO safety_records (id, owner_id, name, amount) VALUES ('two', 'owner', 'name', 2.5)";

    // Removing the composite uniqueness.
    let mut relaxed = SafetyRecord::metadata().clone();
    relaxed.composite_unique_indexes = Box::new([]);
    let relaxed: &'static EntityMetadata = Box::leak(Box::new(relaxed));
    let remove = manager
        .plan_migration_to_entities("v2", "remove uniqueness", &[relaxed])
        .await?;
    assert!(
        remove
            .steps
            .iter()
            .any(|step| step.risk == MigrationRisk::Destructive),
        "{:?}",
        remove.steps
    );
    let refused = manager
        .apply_migration(&remove, ApplyOptions::default())
        .await
        .expect_err("removing uniqueness needs allow_destructive");
    assert!(refused.to_string().contains("destructive"), "{refused}");
    sqlx::query(duplicate)
        .execute(db.pool())
        .await
        .expect_err("the uniqueness is still enforced");
    manager
        .apply_migration(
            &remove,
            ApplyOptions {
                allow_destructive: true,
                ..ApplyOptions::default()
            },
        )
        .await?;
    sqlx::query(duplicate).execute(db.pool()).await?;

    // Narrowing the float column to an integer rounds stored values.
    #[cfg(feature = "postgres")]
    {
        let current = SchemaModel::from_entities(&[relaxed]);
        let mut target = current.clone();
        for column in &mut target.tables[0].columns {
            if column.name == "amount" {
                column.sql_type = "INTEGER".into();
            }
        }
        let narrow = manager.plan_migration("v3", "narrow amount", &current, &target)?;
        assert!(
            narrow
                .steps
                .iter()
                .any(|step| step.risk == MigrationRisk::Destructive),
            "{:?}",
            narrow.steps
        );
        let unchecked = |allow_destructive| ApplyOptions {
            allow_destructive,
            require_clean_schema: false,
            ..ApplyOptions::default()
        };
        manager
            .apply_migration(&narrow, unchecked(false))
            .await
            .expect_err("a lossy type change needs allow_destructive");
        let stored: f64 = sqlx::query_scalar("SELECT amount FROM safety_records WHERE id = 'one'")
            .fetch_one(db.pool())
            .await?;
        assert_eq!(stored, 1.9, "the refused plan changed nothing");
        manager.apply_migration(&narrow, unchecked(true)).await?;
        let stored: i32 = sqlx::query_scalar("SELECT amount FROM safety_records WHERE id = 'one'")
            .fetch_one(db.pool())
            .await?;
        assert_eq!(stored, 2, "the opted-in change rounds the stored value");
        server.cleanup()?;
    }
    Ok(())
}
