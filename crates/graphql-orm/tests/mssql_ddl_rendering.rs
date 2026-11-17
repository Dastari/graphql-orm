//! Rendered SQL Server DDL. Rendering needs no server and no `mssql` feature;
//! `mssql_correctness.rs` executes the same statements on an owned server.
#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]

use graphql_orm::graphql::orm::{
    ColumnModel, DatabaseBackend, DeletePolicy, ForeignKeyModel, MigrationStep,
    render_migration_step,
};

fn column(name: &str, sql_type: &str, nullable: bool) -> ColumnModel {
    ColumnModel {
        name: name.into(),
        sql_type: sql_type.into(),
        spatial: None,
        nullable,
        is_primary_key: false,
        is_unique: false,
        default: None,
    }
}

fn add_foreign_key(on_delete: DeletePolicy) -> MigrationStep {
    MigrationStep::AddForeignKey {
        table_name: "children".into(),
        foreign_key: ForeignKeyModel::single("parent_id", "parents", "id", true, on_delete),
    }
}

#[test]
fn sql_server_spells_restrict_as_no_action() {
    let restrict = add_foreign_key(DeletePolicy::Restrict);
    let mssql = render_migration_step(DatabaseBackend::Mssql, &restrict).join("; ");
    assert!(mssql.ends_with("ON DELETE NO ACTION"), "{mssql}");
    assert!(!mssql.contains("RESTRICT"), "{mssql}");
    let postgres = render_migration_step(DatabaseBackend::Postgres, &restrict).join("; ");
    assert!(postgres.ends_with("ON DELETE RESTRICT"), "{postgres}");

    for (policy, action) in [
        (DeletePolicy::Cascade, "CASCADE"),
        (DeletePolicy::SetNull, "SET NULL"),
    ] {
        let sql = render_migration_step(DatabaseBackend::Mssql, &add_foreign_key(policy)).join("");
        assert!(sql.ends_with(&format!("ON DELETE {action}")), "{sql}");
    }
    assert_eq!(
        DeletePolicy::Restrict.as_sql_for(DatabaseBackend::Sqlite),
        "RESTRICT"
    );
    // The backend-neutral spelling that schema hashes use is unchanged.
    assert_eq!(DeletePolicy::Restrict.as_sql(), "RESTRICT");
}

#[test]
fn sql_server_inline_foreign_keys_use_no_action() {
    let mut table = graphql_orm::graphql::orm::TableModel {
        entity_name: "Child".into(),
        table_name: "children".into(),
        primary_key: "id".into(),
        primary_keys: vec!["id".into()],
        default_sort: "id ASC".into(),
        columns: vec![
            column("id", "BIGINT", false),
            column("parent_id", "BIGINT", false),
        ],
        indexes: Vec::new(),
        composite_unique_indexes: Vec::new(),
        foreign_keys: vec![ForeignKeyModel::single(
            "parent_id",
            "parents",
            "id",
            true,
            DeletePolicy::Restrict,
        )],
        search_indexes: Vec::new(),
        append_only: false,
        retention_purge: false,
        check_constraints: Vec::new(),
    };
    table.columns[0].is_primary_key = true;
    let step = MigrationStep::CreateTable(table);
    let mssql = render_migration_step(DatabaseBackend::Mssql, &step).join("; ");
    assert!(mssql.contains("ON DELETE NO ACTION"), "{mssql}");
    assert!(!mssql.contains("RESTRICT"), "{mssql}");
    let sqlite = render_migration_step(DatabaseBackend::Sqlite, &step).join("; ");
    assert!(sqlite.contains("ON DELETE RESTRICT"), "{sqlite}");
}

#[test]
fn composite_unique_batches_do_not_share_variable_names() {
    let step = |table: &str| MigrationStep::SetCompositeUniques {
        table_name: table.into(),
        before: vec![vec!["owner_id".into(), "name".into()]],
        after: vec![vec!["owner_id".into(), "slug".into()]],
    };
    let statements = [step("dbo.first"), step("dbo.second")]
        .iter()
        .flat_map(|step| render_migration_step(DatabaseBackend::Mssql, step))
        .collect::<Vec<_>>();
    assert_eq!(statements.len(), 4, "{statements:?}");
    // Joined into one batch, no statement may declare a batch-level variable:
    // a second `DECLARE @sql` is an error in SQL Server.
    for statement in &statements {
        assert!(!statement.starts_with("DECLARE"), "{statement}");
    }
    let drops = statements
        .iter()
        .filter(|statement| statement.contains("DROP CONSTRAINT"))
        .collect::<Vec<_>>();
    assert_eq!(drops.len(), 2);
    for drop in drops {
        assert!(
            drop.starts_with("EXEC sys.sp_executesql N'DECLARE @sql"),
            "{drop}"
        );
        // The nested batch is one string literal: every inner quote is doubled.
        let body = drop
            .strip_prefix("EXEC sys.sp_executesql N'")
            .and_then(|rest| rest.strip_suffix('\''))
            .expect("one literal");
        assert!(!body.replace("''", "").contains('\''), "{drop}");
    }
}

#[test]
fn sql_server_column_changes_are_valid_t_sql() {
    let add = MigrationStep::AddColumn {
        table_name: "dbo.records".into(),
        column: column("note", "NVARCHAR(200)", true),
    };
    assert_eq!(
        render_migration_step(DatabaseBackend::Mssql, &add),
        ["ALTER TABLE dbo.records ADD note NVARCHAR(200)"]
    );
    // PostgreSQL keeps the `COLUMN` keyword.
    assert_eq!(
        render_migration_step(DatabaseBackend::Postgres, &add),
        ["ALTER TABLE dbo.records ADD COLUMN note NVARCHAR(200)"]
    );

    // A type without characters has no collation: the statement is plain.
    let retype = |nullable| MigrationStep::AlterColumn {
        table_name: "dbo.records".into(),
        before: column("total", "INT", nullable),
        after: column("total", "BIGINT", nullable),
    };
    assert_eq!(
        render_migration_step(DatabaseBackend::Mssql, &retype(false)),
        ["ALTER TABLE dbo.records ALTER COLUMN total BIGINT NOT NULL"]
    );
    assert_eq!(
        render_migration_step(DatabaseBackend::Mssql, &retype(true)),
        ["ALTER TABLE dbo.records ALTER COLUMN total BIGINT NULL"]
    );

    // A character column keeps its live collation (#742): the statement reads
    // it when it runs and names it unless it is the database default.
    let alter = |nullable| MigrationStep::AlterColumn {
        table_name: "dbo.records".into(),
        before: column("note", "NVARCHAR(100)", nullable),
        after: column("note", "NVARCHAR(200)", nullable),
    };
    for (nullable, nullability) in [(false, "NOT NULL"), (true, "NULL")] {
        let rendered = render_migration_step(DatabaseBackend::Mssql, &alter(nullable));
        let [statement] = rendered.as_slice() else {
            panic!("one statement: {rendered:?}");
        };
        assert!(
            statement.starts_with("EXEC sys.sp_executesql N'"),
            "{statement}"
        );
        for part in [
            "ALTER TABLE dbo.records ALTER COLUMN note NVARCHAR(200)",
            "c.collation_name",
            "OBJECT_ID(N''dbo.records'')",
            "c.name = N''note''",
            "DATABASEPROPERTYEX(DB_NAME(), ''Collation'')",
            " COLLATE ",
        ] {
            assert!(statement.contains(part), "{part}: {statement}");
        }
        assert!(
            statement.contains(&format!("N'' {nullability}''")),
            "{statement}"
        );
    }
}

#[test]
fn sql_server_search_metadata_is_t_sql() {
    use graphql_orm::graphql::orm::{SearchIndexModel, SearchIndexStrategy};
    let step = MigrationStep::CreateSearchIndex {
        table_name: "dbo.records".into(),
        index: SearchIndexModel {
            name: "search_records".into(),
            table_name: "dbo.records".into(),
            entity_name: "Record's".into(),
            primary_key: "id".into(),
            strategy: SearchIndexStrategy::MssqlFullText,
            language: "english".into(),
            tokenizer: "unicode61".into(),
            min_token_len: 2,
            fallback_enabled: false,
            fields: Vec::new(),
            json_paths: Vec::new(),
            relations: Vec::new(),
            readiness_tracked: false,
        },
    };
    let statements = render_migration_step(DatabaseBackend::Mssql, &step);
    let sql = statements.join("\n");
    assert!(!sql.contains("IF NOT EXISTS"), "{sql}");
    assert!(!sql.contains("OR REPLACE"), "{sql}");
    assert!(
        sql.contains("IF OBJECT_ID(N'__graphql_orm_search_metadata', N'U') IS NULL CREATE TABLE"),
        "{sql}"
    );
    assert!(
        sql.contains("entity_name NVARCHAR(450) NOT NULL PRIMARY KEY"),
        "{sql}"
    );
    assert!(sql.contains("WHERE entity_name = N'Record''s'"), "{sql}");
    assert!(sql.contains("IF @@ROWCOUNT = 0 INSERT INTO"), "{sql}");
    // The other backends keep their own statements.
    let sqlite = render_migration_step(DatabaseBackend::Sqlite, &step).join("\n");
    assert!(sqlite.contains("INSERT OR REPLACE INTO"), "{sqlite}");
}
