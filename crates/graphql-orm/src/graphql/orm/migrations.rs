use super::core::{
    ColumnModel, DeletePolicy, ForeignKeyModel, IndexDef, MigrationPlan, MigrationStep, SchemaDiff,
    SchemaModel, TableModel,
};
use super::dialect::DatabaseBackend;
use super::query::PoolProvider;
use sqlx::Row;

fn is_internal_graphql_orm_table(table_name: &str) -> bool {
    table_name.starts_with("__graphql_orm_")
}

fn render_default_clause(backend: DatabaseBackend, default: &str) -> String {
    if backend != DatabaseBackend::Sqlite {
        return default.to_string();
    }

    let trimmed = default.trim();
    if trimmed.starts_with('(') && trimmed.ends_with(')') {
        return trimmed.to_string();
    }

    let uppercase = trimmed.to_ascii_uppercase();
    let is_keyword_default = matches!(
        uppercase.as_str(),
        "CURRENT_TIMESTAMP" | "CURRENT_DATE" | "CURRENT_TIME" | "NULL" | "TRUE" | "FALSE"
    );
    let is_numeric_literal = trimmed
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_digit() || c == '-' || c == '+');
    let is_string_literal = trimmed.starts_with('\'') || uppercase.starts_with("X'");

    if is_keyword_default || is_numeric_literal || is_string_literal {
        trimmed.to_string()
    } else {
        format!("({trimmed})")
    }
}

fn render_column_definition(backend: DatabaseBackend, column: &ColumnModel) -> String {
    let mut parts = vec![format!("{} {}", column.name, column.sql_type)];
    if !column.nullable {
        parts.push("NOT NULL".to_string());
    }
    if column.is_primary_key {
        parts.push("PRIMARY KEY".to_string());
    }
    if column.is_unique && !column.is_primary_key {
        parts.push("UNIQUE".to_string());
    }
    if let Some(default) = &column.default {
        parts.push(format!(
            "DEFAULT {}",
            render_default_clause(backend, default)
        ));
    }
    parts.join(" ")
}

fn render_create_table_statement(backend: DatabaseBackend, table: &TableModel) -> String {
    render_create_table_statement_for_name(backend, table, &table.table_name)
}

fn render_create_table_statement_for_name(
    backend: DatabaseBackend,
    table: &TableModel,
    table_name: &str,
) -> String {
    let mut parts = table
        .columns
        .iter()
        .map(|column| render_column_definition(backend, column))
        .collect::<Vec<_>>();
    parts.extend(table.foreign_keys.iter().map(|foreign_key| {
        let constraint_name = foreign_key_constraint_name(table_name, foreign_key);
        format!(
            "CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {}({}) ON DELETE {}",
            constraint_name,
            foreign_key.source_column,
            foreign_key.target_table,
            foreign_key.target_column,
            foreign_key.on_delete.as_sql(),
        )
    }));
    format!("CREATE TABLE {} ({})", table_name, parts.join(", "))
}

fn column_changed(before: &ColumnModel, after: &ColumnModel) -> bool {
    before != after
}

fn order_tables_by_foreign_keys<'a>(
    tables: &std::collections::BTreeMap<String, &'a TableModel>,
) -> Vec<&'a TableModel> {
    fn visit<'a>(
        table_name: &str,
        tables: &std::collections::BTreeMap<String, &'a TableModel>,
        visiting: &mut std::collections::BTreeSet<String>,
        visited: &mut std::collections::BTreeSet<String>,
        ordered: &mut Vec<&'a TableModel>,
    ) {
        if visited.contains(table_name) || visiting.contains(table_name) {
            return;
        }
        let Some(table) = tables.get(table_name) else {
            return;
        };

        visiting.insert(table_name.to_string());
        for foreign_key in &table.foreign_keys {
            if tables.contains_key(&foreign_key.target_table) {
                visit(
                    &foreign_key.target_table,
                    tables,
                    visiting,
                    visited,
                    ordered,
                );
            }
        }
        visiting.remove(table_name);
        visited.insert(table_name.to_string());
        ordered.push(*table);
    }

    let mut ordered = Vec::new();
    let mut visiting = std::collections::BTreeSet::new();
    let mut visited = std::collections::BTreeSet::new();
    for table_name in tables.keys() {
        visit(
            table_name,
            tables,
            &mut visiting,
            &mut visited,
            &mut ordered,
        );
    }
    ordered
}

pub fn diff_schema_models(current: &SchemaModel, target: &SchemaModel) -> SchemaDiff {
    let current_tables = current
        .tables
        .iter()
        .map(|table| (table.table_name.clone(), table))
        .collect::<std::collections::BTreeMap<_, _>>();
    let target_tables = target
        .tables
        .iter()
        .map(|table| (table.table_name.clone(), table))
        .collect::<std::collections::BTreeMap<_, _>>();

    let mut steps = Vec::new();

    for table in order_tables_by_foreign_keys(&target_tables) {
        let table_name = &table.table_name;
        if !current_tables.contains_key(table_name) {
            steps.push(MigrationStep::CreateTable(table.clone()));
        }
    }

    for table in order_tables_by_foreign_keys(&current_tables)
        .into_iter()
        .rev()
    {
        let table_name = &table.table_name;
        if !target_tables.contains_key(table_name) {
            steps.push(MigrationStep::DropTable {
                table_name: table_name.clone(),
            });
            continue;
        }

        let target_table = target_tables[table_name];
        let current_columns = table
            .columns
            .iter()
            .map(|column| (column.name.clone(), column))
            .collect::<std::collections::BTreeMap<_, _>>();
        let target_columns = target_table
            .columns
            .iter()
            .map(|column| (column.name.clone(), column))
            .collect::<std::collections::BTreeMap<_, _>>();

        for (column_name, column) in &target_columns {
            if !current_columns.contains_key(column_name) {
                steps.push(MigrationStep::AddColumn {
                    table_name: table_name.clone(),
                    column: (*column).clone(),
                });
            }
        }

        for (column_name, column) in &current_columns {
            if !target_columns.contains_key(column_name) {
                steps.push(MigrationStep::DropColumn {
                    table_name: table_name.clone(),
                    column_name: column_name.clone(),
                });
                continue;
            }

            let target_column = target_columns[column_name];
            if column_changed(column, target_column) {
                steps.push(MigrationStep::AlterColumn {
                    table_name: table_name.clone(),
                    before: (*column).clone(),
                    after: (*target_column).clone(),
                });
            }
        }

        let current_indexes = table
            .indexes
            .iter()
            .map(|index| (index.name.to_string(), index))
            .collect::<std::collections::BTreeMap<_, _>>();
        let target_indexes = target_table
            .indexes
            .iter()
            .map(|index| (index.name.to_string(), index))
            .collect::<std::collections::BTreeMap<_, _>>();

        for (index_name, index) in &target_indexes {
            if !current_indexes.contains_key(index_name) {
                steps.push(MigrationStep::CreateIndex {
                    table_name: table_name.clone(),
                    index: (*index).clone(),
                });
            }
        }

        for index_name in current_indexes.keys() {
            if !target_indexes.contains_key(index_name) {
                steps.push(MigrationStep::DropIndex {
                    table_name: table_name.clone(),
                    index_name: index_name.clone(),
                });
            }
        }

        let current_foreign_keys = table
            .foreign_keys
            .iter()
            .map(|foreign_key| {
                (
                    (
                        foreign_key.source_column.clone(),
                        foreign_key.target_table.clone(),
                        foreign_key.target_column.clone(),
                        foreign_key.on_delete.clone(),
                    ),
                    foreign_key,
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let target_foreign_keys = target_table
            .foreign_keys
            .iter()
            .map(|foreign_key| {
                (
                    (
                        foreign_key.source_column.clone(),
                        foreign_key.target_table.clone(),
                        foreign_key.target_column.clone(),
                        foreign_key.on_delete.clone(),
                    ),
                    foreign_key,
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();

        for (key, foreign_key) in &current_foreign_keys {
            if !target_foreign_keys.contains_key(key) {
                steps.push(MigrationStep::DropForeignKey {
                    table_name: table_name.clone(),
                    foreign_key: (*foreign_key).clone(),
                });
            }
        }

        for (key, foreign_key) in &target_foreign_keys {
            if !current_foreign_keys.contains_key(key) {
                steps.push(MigrationStep::AddForeignKey {
                    table_name: table_name.clone(),
                    foreign_key: (*foreign_key).clone(),
                });
            }
        }
    }

    SchemaDiff { steps }
}

fn foreign_key_constraint_name(table_name: &str, foreign_key: &ForeignKeyModel) -> String {
    format!(
        "fk_{}_{}_{}_{}",
        table_name, foreign_key.source_column, foreign_key.target_table, foreign_key.target_column
    )
}

fn migration_step_table_name(step: &MigrationStep) -> Option<&str> {
    match step {
        MigrationStep::CreateTable(table) => Some(&table.table_name),
        MigrationStep::DropTable { table_name } => Some(table_name),
        MigrationStep::AddColumn { table_name, .. } => Some(table_name),
        MigrationStep::DropColumn { table_name, .. } => Some(table_name),
        MigrationStep::AlterColumn { table_name, .. } => Some(table_name),
        MigrationStep::CreateIndex { table_name, .. } => Some(table_name),
        MigrationStep::DropIndex { table_name, .. } => Some(table_name),
        MigrationStep::AddForeignKey { table_name, .. } => Some(table_name),
        MigrationStep::DropForeignKey { table_name, .. } => Some(table_name),
    }
}

fn sqlite_requires_table_rebuild(step: &MigrationStep) -> bool {
    matches!(
        step,
        MigrationStep::DropColumn { .. }
            | MigrationStep::AlterColumn { .. }
            | MigrationStep::AddForeignKey { .. }
            | MigrationStep::DropForeignKey { .. }
    )
}

fn render_sqlite_table_rebuild_statements(
    current_table: &TableModel,
    target_table: &TableModel,
) -> Vec<String> {
    let temp_table_name = format!("__graphql_orm_{}_new", target_table.table_name);
    let target_table_sql = render_create_table_statement_for_name(
        DatabaseBackend::Sqlite,
        target_table,
        &temp_table_name,
    );
    let common_columns = target_table
        .columns
        .iter()
        .filter(|target_column| {
            current_table
                .columns
                .iter()
                .any(|current_column| current_column.name == target_column.name)
        })
        .map(|column| column.name.clone())
        .collect::<Vec<_>>();

    let mut statements = vec![target_table_sql];
    if !common_columns.is_empty() {
        let columns = common_columns.join(", ");
        statements.push(format!(
            "INSERT INTO {} ({}) SELECT {} FROM {}",
            temp_table_name, columns, columns, current_table.table_name
        ));
    }
    statements.push(format!("DROP TABLE {}", current_table.table_name));
    statements.push(format!(
        "ALTER TABLE {} RENAME TO {}",
        temp_table_name, target_table.table_name
    ));
    statements.extend(target_table.indexes.iter().map(|index| {
        let unique = if index.is_unique { "UNIQUE " } else { "" };
        format!(
            "CREATE {}INDEX {} ON {} ({})",
            unique,
            index.name,
            target_table.table_name,
            index.columns.join(", ")
        )
    }));
    statements
}

pub fn render_migration_step(backend: DatabaseBackend, step: &MigrationStep) -> Vec<String> {
    match step {
        MigrationStep::CreateTable(table) => {
            let mut statements = vec![render_create_table_statement(backend, table)];
            statements.extend(table.indexes.iter().map(|index| {
                let unique = if index.is_unique { "UNIQUE " } else { "" };
                format!(
                    "CREATE {}INDEX {} ON {} ({})",
                    unique,
                    index.name,
                    table.table_name,
                    index.columns.join(", ")
                )
            }));
            statements
        }
        MigrationStep::DropTable { table_name } => {
            vec![format!("DROP TABLE {}", table_name)]
        }
        MigrationStep::AddColumn { table_name, column } => vec![format!(
            "ALTER TABLE {} ADD COLUMN {}",
            table_name,
            render_column_definition(backend, column)
        )],
        MigrationStep::DropColumn {
            table_name,
            column_name,
        } => match backend {
            DatabaseBackend::Postgres => vec![format!(
                "ALTER TABLE {} DROP COLUMN {}",
                table_name, column_name
            )],
            DatabaseBackend::Sqlite => vec![format!(
                "-- sqlite requires table rebuild to drop column {} from {}",
                column_name, table_name
            )],
            DatabaseBackend::Mysql | DatabaseBackend::Mssql => vec![format!(
                "ALTER TABLE {} DROP COLUMN {}",
                table_name, column_name
            )],
        },
        MigrationStep::AlterColumn {
            table_name,
            before: _,
            after,
        } => match backend {
            DatabaseBackend::Postgres => vec![
                format!(
                    "ALTER TABLE {} ALTER COLUMN {} TYPE {}",
                    table_name, after.name, after.sql_type
                ),
                if after.nullable {
                    format!(
                        "ALTER TABLE {} ALTER COLUMN {} DROP NOT NULL",
                        table_name, after.name
                    )
                } else {
                    format!(
                        "ALTER TABLE {} ALTER COLUMN {} SET NOT NULL",
                        table_name, after.name
                    )
                },
            ],
            DatabaseBackend::Sqlite => vec![format!(
                "-- sqlite requires table rebuild to alter column {} on {}",
                after.name, table_name
            )],
            DatabaseBackend::Mysql | DatabaseBackend::Mssql => vec![format!(
                "ALTER TABLE {} ALTER COLUMN {} {}",
                table_name, after.name, after.sql_type
            )],
        },
        MigrationStep::CreateIndex { table_name, index } => {
            let unique = if index.is_unique { "UNIQUE " } else { "" };
            vec![format!(
                "CREATE {}INDEX {} ON {} ({})",
                unique,
                index.name,
                table_name,
                index.columns.join(", ")
            )]
        }
        MigrationStep::DropIndex {
            table_name,
            index_name,
        } => match backend {
            DatabaseBackend::Sqlite => {
                vec![format!("DROP INDEX {}", index_name)]
            }
            DatabaseBackend::Postgres => {
                vec![format!("DROP INDEX {}", index_name)]
            }
            DatabaseBackend::Mysql => {
                vec![format!("DROP INDEX {} ON {}", index_name, table_name)]
            }
            DatabaseBackend::Mssql => {
                vec![format!("DROP INDEX {} ON {}", index_name, table_name)]
            }
        },
        MigrationStep::AddForeignKey {
            table_name,
            foreign_key,
        } => {
            let constraint_name = foreign_key_constraint_name(table_name, foreign_key);
            match backend {
                DatabaseBackend::Postgres => vec![format!(
                    "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {}({}) ON DELETE {}",
                    table_name,
                    constraint_name,
                    foreign_key.source_column,
                    foreign_key.target_table,
                    foreign_key.target_column,
                    foreign_key.on_delete.as_sql()
                )],
                DatabaseBackend::Sqlite => vec![format!(
                    "-- sqlite requires table rebuild to add foreign key {} on {}",
                    constraint_name, table_name
                )],
                DatabaseBackend::Mysql | DatabaseBackend::Mssql => vec![format!(
                    "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {}({}) ON DELETE {}",
                    table_name,
                    constraint_name,
                    foreign_key.source_column,
                    foreign_key.target_table,
                    foreign_key.target_column,
                    foreign_key.on_delete.as_sql()
                )],
            }
        }
        MigrationStep::DropForeignKey {
            table_name,
            foreign_key,
        } => {
            let constraint_name = foreign_key_constraint_name(table_name, foreign_key);
            match backend {
                DatabaseBackend::Postgres => vec![format!(
                    "ALTER TABLE {} DROP CONSTRAINT {}",
                    table_name, constraint_name
                )],
                DatabaseBackend::Sqlite => vec![format!(
                    "-- sqlite requires table rebuild to drop foreign key {} on {}",
                    constraint_name, table_name
                )],
                DatabaseBackend::Mysql => vec![format!(
                    "ALTER TABLE {} DROP FOREIGN KEY {}",
                    table_name, constraint_name
                )],
                DatabaseBackend::Mssql => vec![format!(
                    "ALTER TABLE {} DROP CONSTRAINT {}",
                    table_name, constraint_name
                )],
            }
        }
    }
}

pub fn build_migration_plan(
    backend: DatabaseBackend,
    current: &SchemaModel,
    target: &SchemaModel,
) -> MigrationPlan {
    let diff = diff_schema_models(current, target);
    let statements = match backend {
        DatabaseBackend::Sqlite => {
            let current_tables = current
                .tables
                .iter()
                .map(|table| (table.table_name.as_str(), table))
                .collect::<std::collections::BTreeMap<_, _>>();
            let target_tables = target
                .tables
                .iter()
                .map(|table| (table.table_name.as_str(), table))
                .collect::<std::collections::BTreeMap<_, _>>();
            let rebuild_tables = diff
                .steps
                .iter()
                .filter(|step| sqlite_requires_table_rebuild(step))
                .filter_map(migration_step_table_name)
                .collect::<std::collections::BTreeSet<_>>();
            let mut statements = Vec::new();
            let mut rebuilt_tables = std::collections::BTreeSet::new();

            if !rebuild_tables.is_empty() {
                statements.push("PRAGMA foreign_keys = OFF".to_string());
            }

            for step in &diff.steps {
                let table_name = migration_step_table_name(step);
                if let Some(table_name) = table_name {
                    if rebuild_tables.contains(table_name) {
                        if rebuilt_tables.insert(table_name.to_string()) {
                            if let (Some(current_table), Some(target_table)) = (
                                current_tables.get(table_name),
                                target_tables.get(table_name),
                            ) {
                                statements.extend(render_sqlite_table_rebuild_statements(
                                    current_table,
                                    target_table,
                                ));
                            }
                        }
                        continue;
                    }
                }
                statements.extend(render_migration_step(backend, step));
            }

            if !rebuild_tables.is_empty() {
                statements.push("PRAGMA foreign_keys = ON".to_string());
            }

            statements
        }
        _ => diff
            .steps
            .iter()
            .flat_map(|step| render_migration_step(backend, step))
            .collect::<Vec<_>>(),
    };

    MigrationPlan {
        backend,
        steps: diff.steps,
        statements,
    }
}

pub fn migration_filename(version: &str, description: &str) -> String {
    let normalized = description
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    format!("{}_{}.sql", version, normalized)
}

pub fn render_migration_file(plan: &MigrationPlan, version: &str, description: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("-- version: {}\n", version));
    out.push_str(&format!("-- description: {}\n", description));
    out.push_str(&format!("-- backend: {:?}\n\n", plan.backend));
    for statement in &plan.statements {
        out.push_str(statement);
        if !statement.trim_end().ends_with(';') {
            out.push(';');
        }
        out.push('\n');
    }
    out
}

pub fn write_migration_file(
    directory: impl AsRef<std::path::Path>,
    plan: &MigrationPlan,
    version: &str,
    description: &str,
) -> Result<std::path::PathBuf, std::io::Error> {
    let path = directory
        .as_ref()
        .join(migration_filename(version, description));
    std::fs::create_dir_all(directory.as_ref())?;
    std::fs::write(&path, render_migration_file(plan, version, description))?;
    Ok(path)
}

#[cfg(feature = "sqlite")]
pub async fn introspect_schema(provider: &impl PoolProvider) -> Result<SchemaModel, sqlx::Error> {
    let pool = provider.pool();
    let table_rows = sqlx::query(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .fetch_all(pool)
    .await?;

    let mut tables = Vec::new();
    for row in table_rows {
        let table_name: String = row.try_get("name")?;
        if is_internal_graphql_orm_table(&table_name) {
            continue;
        }

        let pragma_table_info = format!("PRAGMA table_info({})", table_name);
        let column_rows = sqlx::query(&pragma_table_info).fetch_all(pool).await?;
        let columns = column_rows
            .into_iter()
            .map(|row| {
                let name: String = row.try_get("name")?;
                let sql_type: String = row.try_get("type")?;
                let nullable = row.try_get::<i64, _>("notnull")? == 0;
                let default = row.try_get::<Option<String>, _>("dflt_value")?;
                let is_primary_key = row.try_get::<i64, _>("pk")? > 0;
                Ok(ColumnModel {
                    name,
                    sql_type,
                    nullable,
                    is_primary_key,
                    is_unique: false,
                    default,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        let primary_key = columns
            .iter()
            .find(|column| column.is_primary_key)
            .map(|column| column.name.clone())
            .unwrap_or_else(|| "id".to_string());

        let pragma_index_list = format!("PRAGMA index_list({})", table_name);
        let index_rows = sqlx::query(&pragma_index_list).fetch_all(pool).await?;
        let mut indexes = Vec::new();
        for row in index_rows {
            let index_name: String = row.try_get("name")?;
            if index_name.starts_with("sqlite_autoindex_") {
                continue;
            }
            let unique = row.try_get::<i64, _>("unique")? != 0;
            let pragma_index_info = format!("PRAGMA index_info({})", index_name);
            let index_info_rows = sqlx::query(&pragma_index_info).fetch_all(pool).await?;
            let column_names = index_info_rows
                .into_iter()
                .map(|index_row| index_row.try_get::<String, _>("name"))
                .collect::<Result<Vec<_>, _>>()?;
            let leaked_name: &'static str = Box::leak(index_name.into_boxed_str());
            let leaked_columns: &'static [&'static str] = Box::leak(
                column_names
                    .into_iter()
                    .map(|column| Box::leak(column.into_boxed_str()) as &'static str)
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            );
            indexes.push(IndexDef {
                name: leaked_name,
                columns: leaked_columns,
                is_unique: unique,
            });
        }

        let pragma_fk_list = format!("PRAGMA foreign_key_list({})", table_name);
        let foreign_key_rows = sqlx::query(&pragma_fk_list).fetch_all(pool).await?;
        let foreign_keys = foreign_key_rows
            .into_iter()
            .map(|row| {
                Ok(ForeignKeyModel {
                    source_column: row.try_get("from")?,
                    target_table: row.try_get("table")?,
                    target_column: row.try_get("to")?,
                    is_multiple: false,
                    on_delete: match row.try_get::<String, _>("on_delete")?.as_str() {
                        "CASCADE" => DeletePolicy::Cascade,
                        "SET NULL" => DeletePolicy::SetNull,
                        _ => DeletePolicy::Restrict,
                    },
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;

        tables.push(TableModel {
            entity_name: table_name.clone(),
            table_name,
            primary_key: primary_key.clone(),
            default_sort: primary_key.clone(),
            columns,
            indexes,
            composite_unique_indexes: Vec::new(),
            foreign_keys,
        });
    }

    Ok(SchemaModel { tables })
}

#[cfg(feature = "postgres")]
pub async fn introspect_schema(provider: &impl PoolProvider) -> Result<SchemaModel, sqlx::Error> {
    let pool = provider.pool();
    let table_rows = sqlx::query(
        "SELECT table_name
         FROM information_schema.tables
         WHERE table_schema = 'public' AND table_type = 'BASE TABLE'
         ORDER BY table_name",
    )
    .fetch_all(pool)
    .await?;

    let mut tables = Vec::new();
    for row in table_rows {
        let table_name: String = row.try_get("table_name")?;
        if is_internal_graphql_orm_table(&table_name) {
            continue;
        }
        let column_rows = sqlx::query(
            "SELECT column_name, data_type, is_nullable, column_default
             FROM information_schema.columns
             WHERE table_schema = 'public' AND table_name = $1
             ORDER BY ordinal_position",
        )
        .bind(&table_name)
        .fetch_all(pool)
        .await?;

        let primary_key_rows = sqlx::query(
            "SELECT kcu.column_name
             FROM information_schema.table_constraints tc
             JOIN information_schema.key_column_usage kcu
               ON tc.constraint_name = kcu.constraint_name
              AND tc.table_schema = kcu.table_schema
             WHERE tc.table_schema = 'public'
               AND tc.table_name = $1
               AND tc.constraint_type = 'PRIMARY KEY'",
        )
        .bind(&table_name)
        .fetch_all(pool)
        .await?;
        let primary_key_columns = primary_key_rows
            .into_iter()
            .map(|row| row.try_get::<String, _>("column_name"))
            .collect::<Result<Vec<_>, _>>()?;

        let unique_rows = sqlx::query(
            "SELECT kcu.column_name
             FROM information_schema.table_constraints tc
             JOIN information_schema.key_column_usage kcu
               ON tc.constraint_name = kcu.constraint_name
              AND tc.table_schema = kcu.table_schema
             WHERE tc.table_schema = 'public'
               AND tc.table_name = $1
               AND tc.constraint_type = 'UNIQUE'",
        )
        .bind(&table_name)
        .fetch_all(pool)
        .await?;
        let unique_columns = unique_rows
            .into_iter()
            .map(|row| row.try_get::<String, _>("column_name"))
            .collect::<Result<std::collections::HashSet<_>, _>>()?;

        let columns = column_rows
            .into_iter()
            .map(|row| {
                let name: String = row.try_get("column_name")?;
                Ok(ColumnModel {
                    is_primary_key: primary_key_columns.iter().any(|column| column == &name),
                    is_unique: unique_columns.contains(&name),
                    name,
                    sql_type: row.try_get::<String, _>("data_type")?,
                    nullable: row.try_get::<String, _>("is_nullable")? == "YES",
                    default: row.try_get::<Option<String>, _>("column_default")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;

        let primary_key = primary_key_columns
            .first()
            .cloned()
            .unwrap_or_else(|| "id".to_string());

        let index_rows = sqlx::query(
            "SELECT indexname, indexdef
             FROM pg_indexes
             WHERE schemaname = 'public' AND tablename = $1",
        )
        .bind(&table_name)
        .fetch_all(pool)
        .await?;
        let mut indexes = Vec::new();
        for row in index_rows {
            let index_name: String = row.try_get("indexname")?;
            if index_name.ends_with("_pkey") {
                continue;
            }
            let indexdef: String = row.try_get("indexdef")?;
            let unique = indexdef.contains("CREATE UNIQUE INDEX");
            let columns_segment = indexdef
                .split('(')
                .nth(1)
                .and_then(|segment| segment.split(')').next())
                .unwrap_or("");
            let column_names = columns_segment
                .split(',')
                .map(str::trim)
                .filter(|column| !column.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>();
            let leaked_name: &'static str = Box::leak(index_name.into_boxed_str());
            let leaked_columns: &'static [&'static str] = Box::leak(
                column_names
                    .into_iter()
                    .map(|column| Box::leak(column.into_boxed_str()) as &'static str)
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            );
            indexes.push(IndexDef {
                name: leaked_name,
                columns: leaked_columns,
                is_unique: unique,
            });
        }

        let foreign_key_rows = sqlx::query(
            "SELECT
                kcu.column_name AS source_column,
                ccu.table_name AS target_table,
                ccu.column_name AS target_column,
                rc.delete_rule AS delete_rule
             FROM information_schema.table_constraints tc
             JOIN information_schema.key_column_usage kcu
               ON tc.constraint_name = kcu.constraint_name
              AND tc.table_schema = kcu.table_schema
             JOIN information_schema.constraint_column_usage ccu
               ON ccu.constraint_name = tc.constraint_name
              AND ccu.table_schema = tc.table_schema
             JOIN information_schema.referential_constraints rc
               ON rc.constraint_name = tc.constraint_name
              AND rc.constraint_schema = tc.table_schema
             WHERE tc.table_schema = 'public'
               AND tc.table_name = $1
               AND tc.constraint_type = 'FOREIGN KEY'",
        )
        .bind(&table_name)
        .fetch_all(pool)
        .await?;
        let foreign_keys = foreign_key_rows
            .into_iter()
            .map(|row| {
                Ok(ForeignKeyModel {
                    source_column: row.try_get("source_column")?,
                    target_table: row.try_get("target_table")?,
                    target_column: row.try_get("target_column")?,
                    is_multiple: false,
                    on_delete: match row.try_get::<String, _>("delete_rule")?.as_str() {
                        "CASCADE" => DeletePolicy::Cascade,
                        "SET NULL" => DeletePolicy::SetNull,
                        _ => DeletePolicy::Restrict,
                    },
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;

        tables.push(TableModel {
            entity_name: table_name.clone(),
            table_name,
            primary_key: primary_key.clone(),
            default_sort: primary_key,
            columns,
            indexes,
            composite_unique_indexes: Vec::new(),
            foreign_keys,
        });
    }

    Ok(SchemaModel { tables })
}
