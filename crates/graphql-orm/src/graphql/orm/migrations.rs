#[cfg(feature = "mssql")]
use super::OrmBackend;
#[cfg(feature = "postgres")]
use super::core::RlsOperation;
#[cfg(feature = "mssql")]
use super::core::SqlValue;
use super::core::{
    ColumnModel, ForeignKeyModel, IndexMethod, MigrationPlan, MigrationRisk, MigrationStep,
    PlannedMigrationStep, SchemaDiff, SchemaModel, SearchIndexModel, SearchIndexStrategy,
    TableModel,
};
#[cfg(any(feature = "sqlite", feature = "postgres"))]
use super::core::{SearchFieldModel, SearchJsonPathModel, SearchRelationFieldModel, SearchWeight};
#[cfg(feature = "postgres")]
use super::core::{SpatialColumnDef, SpatialGeometryType};
use super::dialect::DatabaseBackend;
#[cfg(feature = "mssql")]
use super::dialect::SqlDialect;
use super::query::PoolProvider;
#[cfg(feature = "postgres")]
use super::rls::{LiveRlsPolicy, LiveRlsTable};
use super::{IntrospectionBackend, RlsIntrospectionBackend};
#[cfg(any(feature = "sqlite", feature = "postgres"))]
use sqlx::Row;

#[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
fn is_internal_graphql_orm_table(table_name: &str) -> bool {
    table_name.starts_with("__graphql_orm_") || table_name == "spatial_ref_sys"
}

fn defaults_equivalent(before: &Option<String>, after: &Option<String>) -> bool {
    match (before, after) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            super::dialect::canonicalize_column_default_expression(left)
                == super::dialect::canonicalize_column_default_expression(right)
        }
        _ => false,
    }
}

fn render_default_clause(backend: DatabaseBackend, default: &str) -> String {
    if backend != DatabaseBackend::Sqlite {
        return default.to_string();
    }

    // Render from the canonical form so DDL is stable regardless of whether the
    // metadata stored optional outer parentheses.
    let trimmed = super::dialect::canonicalize_column_default_expression(default);
    let uppercase = trimmed.to_ascii_uppercase();
    let is_keyword_default = matches!(
        uppercase.as_str(),
        "CURRENT_TIMESTAMP" | "CURRENT_DATE" | "CURRENT_TIME" | "NULL" | "TRUE" | "FALSE"
    );
    let is_numeric_literal = trimmed
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_digit() || c == '-' || c == '+')
        && trimmed
            .chars()
            .all(|c| c.is_ascii_digit() || c == '-' || c == '+' || c == '.');
    let is_string_literal = trimmed.starts_with('\'') || uppercase.starts_with("X'");

    if is_keyword_default || is_numeric_literal || is_string_literal {
        trimmed
    } else {
        format!("({trimmed})")
    }
}

fn render_column_definition(
    backend: DatabaseBackend,
    column: &ColumnModel,
    inline_primary_key: bool,
) -> String {
    let mut parts = vec![format!("{} {}", column.name, column.sql_type)];
    if !column.nullable {
        parts.push("NOT NULL".to_string());
    }
    if inline_primary_key && column.is_primary_key {
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
    let has_composite_primary_key = table.primary_keys().len() > 1;
    let mut parts = table
        .columns
        .iter()
        .map(|column| render_column_definition(backend, column, !has_composite_primary_key))
        .collect::<Vec<_>>();
    if has_composite_primary_key {
        parts.push(format!("PRIMARY KEY ({})", table.primary_keys().join(", ")));
    }
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

fn column_changed_for_backend(
    backend: DatabaseBackend,
    before: &ColumnModel,
    after: &ColumnModel,
) -> bool {
    let mut before = before.clone();
    let mut after = after.clone();
    if backend == DatabaseBackend::Sqlite {
        // Spatial metadata is not represented in SQLite DDL the same way as Postgres.
        before.spatial = None;
        after.spatial = None;
    }

    if before.name != after.name
        || before.sql_type != after.sql_type
        || before.nullable != after.nullable
        || before.is_primary_key != after.is_primary_key
        || before.is_unique != after.is_unique
        || before.spatial != after.spatial
    {
        return true;
    }

    !defaults_equivalent(&before.default, &after.default)
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
    diff_schema_models_for_backend(DatabaseBackend::Postgres, current, target)
}

pub fn diff_schema_models_for_backend(
    backend: DatabaseBackend,
    current: &SchemaModel,
    target: &SchemaModel,
) -> SchemaDiff {
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

    let current_extensions = current
        .extensions
        .iter()
        .map(|extension| extension.to_ascii_lowercase())
        .collect::<std::collections::BTreeSet<_>>();
    for extension in &target.extensions {
        if !current_extensions.contains(&extension.to_ascii_lowercase()) {
            steps.push(MigrationStep::EnableExtension {
                name: extension.clone(),
            });
        }
    }

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
            for search_index in &table.search_indexes {
                steps.push(MigrationStep::DropSearchIndex {
                    table_name: table_name.clone(),
                    index_name: search_index.name.clone(),
                });
            }
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
            if column_changed_for_backend(backend, column, target_column) {
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
            if let Some(current_index) = current_indexes.get(index_name) {
                if *current_index != *index {
                    steps.push(MigrationStep::DropIndex {
                        table_name: table_name.clone(),
                        index_name: index_name.clone(),
                    });
                    steps.push(MigrationStep::CreateIndex {
                        table_name: table_name.clone(),
                        index: (*index).clone(),
                    });
                }
            } else {
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

        let current_search_indexes = table
            .search_indexes
            .iter()
            .map(|index| (index.name.clone(), index))
            .collect::<std::collections::BTreeMap<_, _>>();
        let target_search_indexes = target_table
            .search_indexes
            .iter()
            .map(|index| (index.name.clone(), index))
            .collect::<std::collections::BTreeMap<_, _>>();

        for (index_name, index) in &target_search_indexes {
            if let Some(current_index) = current_search_indexes.get(index_name) {
                if *current_index != *index {
                    steps.push(MigrationStep::AlterSearchIndex {
                        table_name: table_name.clone(),
                        before: (*current_index).clone(),
                        after: (*index).clone(),
                    });
                }
            } else {
                steps.push(MigrationStep::CreateSearchIndex {
                    table_name: table_name.clone(),
                    index: (*index).clone(),
                });
            }
        }

        for index_name in current_search_indexes.keys() {
            if !target_search_indexes.contains_key(index_name) {
                steps.push(MigrationStep::DropSearchIndex {
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
        MigrationStep::EnableExtension { .. } => None,
        MigrationStep::CreateTable(table) => Some(&table.table_name),
        MigrationStep::DropTable { table_name } => Some(table_name),
        MigrationStep::AddColumn { table_name, .. } => Some(table_name),
        MigrationStep::DropColumn { table_name, .. } => Some(table_name),
        MigrationStep::AlterColumn { table_name, .. } => Some(table_name),
        MigrationStep::CreateIndex { table_name, .. } => Some(table_name),
        MigrationStep::DropIndex { table_name, .. } => Some(table_name),
        MigrationStep::CreateSearchIndex { table_name, .. } => Some(table_name),
        MigrationStep::DropSearchIndex { table_name, .. } => Some(table_name),
        MigrationStep::AlterSearchIndex { table_name, .. } => Some(table_name),
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

fn render_create_index_statement(
    backend: DatabaseBackend,
    table_name: &str,
    index: &super::core::IndexDef,
) -> String {
    let unique = if index.is_unique { "UNIQUE " } else { "" };
    let method = match (backend, index.method) {
        (DatabaseBackend::Postgres, IndexMethod::Gist) => " USING GIST",
        _ => "",
    };
    format!(
        "CREATE {}INDEX {} ON {}{} ({})",
        unique,
        index.name,
        table_name,
        method,
        index.columns.join(", ")
    )
}

fn sql_string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

fn json_option_string(value: Option<&String>) -> String {
    value
        .map(|value| json_string(value))
        .unwrap_or_else(|| "null".to_string())
}

fn search_index_config_json(index: &SearchIndexModel) -> String {
    let fields = index
        .fields
        .iter()
        .map(|field| {
            format!(
                "{{\"field\":\"{}\",\"column\":\"{}\",\"weight\":\"{}\",\"alias\":{},\"policy\":{}}}",
                field.field_name.replace('"', "\\\""),
                field.column_name.replace('"', "\\\""),
                field.weight.as_str(),
                field
                    .alias
                    .as_ref()
                    .map(|value| format!("\"{}\"", value.replace('"', "\\\"")))
                    .unwrap_or_else(|| "null".to_string()),
                field
                    .policy
                    .as_ref()
                    .map(|value| format!("\"{}\"", value.replace('"', "\\\"")))
                    .unwrap_or_else(|| "null".to_string())
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let json_paths = index
        .json_paths
        .iter()
        .map(|json_path| {
            format!(
                "{{\"field\":\"{}\",\"column\":\"{}\",\"path\":\"{}\",\"weight\":\"{}\",\"policy\":{}}}",
                json_path.field_name.replace('"', "\\\""),
                json_path.column_name.replace('"', "\\\""),
                json_path.path.replace('"', "\\\""),
                json_path.weight.as_str(),
                json_option_string(json_path.policy.as_ref())
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let relations = index
        .relations
        .iter()
        .map(|relation| {
            format!(
                "{{\"relation\":\"{}\",\"target\":\"{}\",\"fields\":[{}],\"weight\":\"{}\",\"max_items\":{},\"policy\":{}}}",
                relation.relation_field.replace('"', "\\\""),
                relation.target_type.replace('"', "\\\""),
                relation
                    .fields
                    .iter()
                    .map(|field| format!("\"{}\"", field.replace('"', "\\\"")))
                    .collect::<Vec<_>>()
                    .join(","),
                relation.weight.as_str(),
                relation.max_items,
                relation
                    .policy
                    .as_ref()
                    .map(|value| format!("\"{}\"", value.replace('"', "\\\"")))
                    .unwrap_or_else(|| "null".to_string())
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"name\":\"{}\",\"table\":\"{}\",\"entity\":\"{}\",\"primary_key\":\"{}\",\"strategy\":\"{}\",\"language\":\"{}\",\"tokenizer\":\"{}\",\"min_token_len\":{},\"fallback_enabled\":{},\"fields\":[{}],\"json_paths\":[{}],\"relations\":[{}]}}",
        index.name.replace('"', "\\\""),
        index.table_name.replace('"', "\\\""),
        index.entity_name.replace('"', "\\\""),
        index.primary_key.replace('"', "\\\""),
        index.strategy.as_str(),
        index.language.replace('"', "\\\""),
        index.tokenizer.replace('"', "\\\""),
        index.min_token_len,
        index.fallback_enabled,
        fields,
        json_paths,
        relations
    )
}

fn render_search_metadata_upsert(
    backend: DatabaseBackend,
    index: &SearchIndexModel,
) -> Vec<String> {
    let metadata_table = super::search_metadata_table_name();
    let now_expr = match backend {
        DatabaseBackend::Postgres => "EXTRACT(EPOCH FROM NOW())::bigint",
        DatabaseBackend::Sqlite => "unixepoch()",
        DatabaseBackend::Mysql => "UNIX_TIMESTAMP()",
        DatabaseBackend::Mssql => "DATEDIFF_BIG(second, '1970-01-01', SYSUTCDATETIME())",
    };
    let create = match backend {
        DatabaseBackend::Postgres => format!(
            "CREATE TABLE IF NOT EXISTS {metadata_table} (entity_name TEXT PRIMARY KEY, table_name TEXT NOT NULL, strategy TEXT NOT NULL, config_json JSONB NOT NULL, updated_at BIGINT NOT NULL)"
        ),
        _ => format!(
            "CREATE TABLE IF NOT EXISTS {metadata_table} (entity_name TEXT PRIMARY KEY, table_name TEXT NOT NULL, strategy TEXT NOT NULL, config_json TEXT NOT NULL, updated_at INTEGER NOT NULL)"
        ),
    };
    let config = search_index_config_json(index);
    let insert = match backend {
        DatabaseBackend::Postgres => format!(
            "INSERT INTO {metadata_table} (entity_name, table_name, strategy, config_json, updated_at) VALUES ({}, {}, {}, {}::jsonb, {now_expr}) ON CONFLICT (entity_name) DO UPDATE SET table_name = EXCLUDED.table_name, strategy = EXCLUDED.strategy, config_json = EXCLUDED.config_json, updated_at = EXCLUDED.updated_at",
            sql_string_literal(&index.entity_name),
            sql_string_literal(&index.table_name),
            sql_string_literal(index.strategy.as_str()),
            sql_string_literal(&config)
        ),
        DatabaseBackend::Sqlite | DatabaseBackend::Mysql | DatabaseBackend::Mssql => format!(
            "INSERT OR REPLACE INTO {metadata_table} (entity_name, table_name, strategy, config_json, updated_at) VALUES ({}, {}, {}, {}, {now_expr})",
            sql_string_literal(&index.entity_name),
            sql_string_literal(&index.table_name),
            sql_string_literal(index.strategy.as_str()),
            sql_string_literal(&config)
        ),
    };
    vec![create, insert]
}

fn render_create_search_index_statement(
    backend: DatabaseBackend,
    index: &SearchIndexModel,
) -> Vec<String> {
    let mut statements = render_search_metadata_upsert(backend, index);
    let table = super::search_table_name(&index.table_name);
    let fts_table = super::sqlite_fts_table_name(&index.table_name);
    let token_table = super::search_token_table_name(&index.table_name);
    match backend {
        DatabaseBackend::Postgres => {
            statements.push(format!(
                "CREATE TABLE {table} (entity_pk TEXT PRIMARY KEY, entity_pk_json JSONB NOT NULL, document_text TEXT NOT NULL, document_vector TSVECTOR NOT NULL, updated_at BIGINT NOT NULL)"
            ));
            statements.push(format!(
                "CREATE INDEX {} ON {table} USING GIN (document_vector)",
                index.name
            ));
        }
        DatabaseBackend::Sqlite => match index.strategy {
            SearchIndexStrategy::FallbackTable => {
                statements.push(format!(
                    "CREATE TABLE {table} (entity_pk TEXT PRIMARY KEY, entity_pk_json TEXT NOT NULL, document_text TEXT NOT NULL, weight_a TEXT NOT NULL, weight_b TEXT NOT NULL, weight_c TEXT NOT NULL, weight_d TEXT NOT NULL, updated_at INTEGER NOT NULL)"
                ));
                statements.push(format!(
                    "CREATE TABLE {token_table} (entity_pk TEXT NOT NULL, token TEXT NOT NULL, weight INTEGER NOT NULL, frequency INTEGER NOT NULL, PRIMARY KEY (entity_pk, token, weight))"
                ));
                statements.push(format!(
                    "CREATE INDEX idx_gom_search_token_{} ON {token_table} (token)",
                    super::sanitize_search_name(&index.table_name)
                ));
            }
            _ => {
                statements.push(format!(
                    "CREATE VIRTUAL TABLE {fts_table} USING fts5(entity_pk UNINDEXED, weight_a, weight_b, weight_c, weight_d, document_text, tokenize = '{}')",
                    index.tokenizer.replace('\'', "''")
                ));
            }
        },
        DatabaseBackend::Mysql | DatabaseBackend::Mssql => {
            statements.push(format!(
                "-- full-text search indexes for {} are planned but not implemented by graphql-orm yet",
                backend.name()
            ));
        }
    }
    statements
}

fn render_drop_search_index_statement(
    backend: DatabaseBackend,
    table_name: &str,
    index_name: &str,
) -> Vec<String> {
    let table = super::search_table_name(table_name);
    let fts_table = super::sqlite_fts_table_name(table_name);
    let token_table = super::search_token_table_name(table_name);
    let metadata_table = super::search_metadata_table_name();
    match backend {
        DatabaseBackend::Postgres => vec![
            format!("DROP INDEX IF EXISTS {index_name}"),
            format!("DROP TABLE IF EXISTS {table}"),
            format!(
                "DELETE FROM {metadata_table} WHERE table_name = {}",
                sql_string_literal(table_name)
            ),
        ],
        DatabaseBackend::Sqlite => vec![
            format!("DROP TABLE IF EXISTS {fts_table}"),
            format!("DROP TABLE IF EXISTS {token_table}"),
            format!("DROP TABLE IF EXISTS {table}"),
            format!(
                "DELETE FROM {metadata_table} WHERE table_name = {}",
                sql_string_literal(table_name)
            ),
        ],
        DatabaseBackend::Mysql | DatabaseBackend::Mssql => vec![format!(
            "-- dropping full-text search index {index_name} for {} is not implemented by graphql-orm yet",
            backend.name()
        )],
    }
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
fn parse_search_weight(value: &str) -> SearchWeight {
    match value {
        "A" => SearchWeight::A,
        "B" => SearchWeight::B,
        "C" => SearchWeight::C,
        _ => SearchWeight::D,
    }
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
fn parse_search_strategy(value: &str) -> SearchIndexStrategy {
    match value {
        "postgres_tsvector" => SearchIndexStrategy::PostgresTsvector,
        "sqlite_fts5" => SearchIndexStrategy::SqliteFts5,
        "mysql_fulltext" => SearchIndexStrategy::MysqlFullText,
        "mssql_fulltext" => SearchIndexStrategy::MssqlFullText,
        _ => SearchIndexStrategy::FallbackTable,
    }
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
fn parse_search_index_config(config_json: &str) -> Option<SearchIndexModel> {
    let value = serde_json::from_str::<serde_json::Value>(config_json).ok()?;
    let fields = value
        .get("fields")
        .and_then(|value| value.as_array())
        .map(|fields| {
            fields
                .iter()
                .filter_map(|field| {
                    Some(SearchFieldModel {
                        field_name: field.get("field")?.as_str()?.to_string(),
                        column_name: field.get("column")?.as_str()?.to_string(),
                        weight: parse_search_weight(
                            field
                                .get("weight")
                                .and_then(|value| value.as_str())
                                .unwrap_or("D"),
                        ),
                        alias: field
                            .get("alias")
                            .and_then(|value| value.as_str())
                            .map(str::to_string),
                        policy: field
                            .get("policy")
                            .and_then(|value| value.as_str())
                            .map(str::to_string),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let relations = value
        .get("relations")
        .and_then(|value| value.as_array())
        .map(|relations| {
            relations
                .iter()
                .filter_map(|relation| {
                    Some(SearchRelationFieldModel {
                        relation_field: relation.get("relation")?.as_str()?.to_string(),
                        target_type: relation.get("target")?.as_str()?.to_string(),
                        fields: relation
                            .get("fields")
                            .and_then(|value| value.as_array())
                            .map(|fields| {
                                fields
                                    .iter()
                                    .filter_map(|field| field.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        weight: parse_search_weight(
                            relation
                                .get("weight")
                                .and_then(|value| value.as_str())
                                .unwrap_or("D"),
                        ),
                        max_items: relation
                            .get("max_items")
                            .and_then(|value| value.as_u64())
                            .unwrap_or(100) as usize,
                        policy: relation
                            .get("policy")
                            .and_then(|value| value.as_str())
                            .map(str::to_string),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let json_paths = value
        .get("json_paths")
        .and_then(|value| value.as_array())
        .map(|json_paths| {
            json_paths
                .iter()
                .filter_map(|json_path| {
                    Some(SearchJsonPathModel {
                        field_name: json_path.get("field")?.as_str()?.to_string(),
                        column_name: json_path.get("column")?.as_str()?.to_string(),
                        path: json_path.get("path")?.as_str()?.to_string(),
                        weight: parse_search_weight(
                            json_path
                                .get("weight")
                                .and_then(|value| value.as_str())
                                .unwrap_or("D"),
                        ),
                        policy: json_path
                            .get("policy")
                            .and_then(|value| value.as_str())
                            .map(str::to_string),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some(SearchIndexModel {
        name: value.get("name")?.as_str()?.to_string(),
        table_name: value.get("table")?.as_str()?.to_string(),
        entity_name: value.get("entity")?.as_str()?.to_string(),
        primary_key: value.get("primary_key")?.as_str()?.to_string(),
        strategy: parse_search_strategy(value.get("strategy")?.as_str()?),
        language: value
            .get("language")
            .and_then(|value| value.as_str())
            .unwrap_or("english")
            .to_string(),
        tokenizer: value
            .get("tokenizer")
            .and_then(|value| value.as_str())
            .unwrap_or("unicode61")
            .to_string(),
        min_token_len: value
            .get("min_token_len")
            .and_then(|value| value.as_u64())
            .unwrap_or(2) as usize,
        fallback_enabled: value
            .get("fallback_enabled")
            .and_then(|value| value.as_bool())
            .unwrap_or(true),
        fields,
        json_paths,
        relations,
    })
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
fn attach_search_indexes(tables: &mut [TableModel], search_indexes: Vec<SearchIndexModel>) {
    for search_index in search_indexes {
        if let Some(table) = tables
            .iter_mut()
            .find(|table| table.table_name == search_index.table_name)
        {
            table.search_indexes.push(search_index);
        }
    }
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
        render_create_index_statement(DatabaseBackend::Sqlite, &target_table.table_name, index)
    }));
    statements
}

pub fn render_migration_step(backend: DatabaseBackend, step: &MigrationStep) -> Vec<String> {
    match step {
        MigrationStep::EnableExtension { name } => match backend {
            DatabaseBackend::Postgres => vec![format!("CREATE EXTENSION IF NOT EXISTS {name}")],
            DatabaseBackend::Sqlite | DatabaseBackend::Mysql | DatabaseBackend::Mssql => {
                vec![format!(
                    "-- extension {} is not supported on {}",
                    name,
                    backend.name()
                )]
            }
        },
        MigrationStep::CreateTable(table) => {
            let mut statements = vec![render_create_table_statement(backend, table)];
            statements.extend(
                table
                    .indexes
                    .iter()
                    .map(|index| render_create_index_statement(backend, &table.table_name, index)),
            );
            statements.extend(
                table
                    .search_indexes
                    .iter()
                    .flat_map(|index| render_create_search_index_statement(backend, index)),
            );
            statements
        }
        MigrationStep::DropTable { table_name } => {
            vec![format!("DROP TABLE {}", table_name)]
        }
        MigrationStep::AddColumn { table_name, column } => vec![format!(
            "ALTER TABLE {} ADD COLUMN {}",
            table_name,
            render_column_definition(backend, column, true)
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
            vec![render_create_index_statement(backend, table_name, index)]
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
        MigrationStep::CreateSearchIndex {
            table_name: _,
            index,
        } => render_create_search_index_statement(backend, index),
        MigrationStep::DropSearchIndex {
            table_name,
            index_name,
        } => render_drop_search_index_statement(backend, table_name, index_name),
        MigrationStep::AlterSearchIndex {
            table_name,
            before,
            after,
        } => {
            let mut statements =
                render_drop_search_index_statement(backend, table_name, &before.name);
            statements.extend(render_create_search_index_statement(backend, after));
            statements
        }
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

pub fn classify_migration_step(step: &MigrationStep) -> PlannedMigrationStep {
    let (risk, reason) = match step {
        MigrationStep::EnableExtension { .. } => (
            MigrationRisk::Additive,
            "enables a database extension without changing row data",
        ),
        MigrationStep::CreateTable(_) => (
            MigrationRisk::Additive,
            "creates a new table without changing existing data",
        ),
        MigrationStep::DropTable { .. } => (
            MigrationRisk::Destructive,
            "drops an existing table and its data",
        ),
        MigrationStep::AddColumn { column, .. } if column.nullable || column.default.is_some() => (
            MigrationRisk::Additive,
            "adds a nullable or defaulted column",
        ),
        MigrationStep::AddColumn { .. } => (
            MigrationRisk::Risky,
            "adds a required column without a default",
        ),
        MigrationStep::DropColumn { .. } => (
            MigrationRisk::Destructive,
            "drops an existing column and its data",
        ),
        MigrationStep::AlterColumn { before, after, .. } => {
            if before.nullable && !after.nullable {
                (
                    MigrationRisk::Risky,
                    "tightens nullability on an existing column",
                )
            } else if before.sql_type == after.sql_type {
                (
                    MigrationRisk::Compatible,
                    "changes column metadata without changing the SQL type",
                )
            } else {
                (MigrationRisk::Risky, "changes an existing column type")
            }
        }
        MigrationStep::CreateIndex { .. } => (
            MigrationRisk::Additive,
            "creates an index without changing row data",
        ),
        MigrationStep::DropIndex { .. } => (MigrationRisk::Risky, "drops an existing index"),
        MigrationStep::CreateSearchIndex { .. } => (
            MigrationRisk::Additive,
            "creates full-text search structures without backfilling row data",
        ),
        MigrationStep::DropSearchIndex { .. } => {
            (MigrationRisk::Risky, "drops full-text search structures")
        }
        MigrationStep::AlterSearchIndex { .. } => (
            MigrationRisk::Risky,
            "recreates full-text search structures and requires an explicit rebuild",
        ),
        MigrationStep::AddForeignKey { .. } => (
            MigrationRisk::Risky,
            "adds a constraint that may reject existing rows",
        ),
        MigrationStep::DropForeignKey { .. } => (
            MigrationRisk::Risky,
            "drops an existing referential constraint",
        ),
    };

    PlannedMigrationStep {
        step: step.clone(),
        risk,
        reason: reason.to_string(),
    }
}

pub fn classify_migration_steps(steps: &[MigrationStep]) -> Vec<PlannedMigrationStep> {
    steps.iter().map(classify_migration_step).collect()
}

pub fn build_migration_plan(
    backend: DatabaseBackend,
    current: &SchemaModel,
    target: &SchemaModel,
) -> MigrationPlan {
    let mut target_for_backend = target.clone();
    if backend != DatabaseBackend::Postgres {
        target_for_backend.extensions.clear();
    }
    let diff = diff_schema_models_for_backend(backend, current, &target_for_backend);
    let statements = match backend {
        DatabaseBackend::Sqlite => {
            let current_tables = current
                .tables
                .iter()
                .map(|table| (table.table_name.as_str(), table))
                .collect::<std::collections::BTreeMap<_, _>>();
            let target_tables = target_for_backend
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

pub async fn introspect_schema<B, P>(provider: &P) -> crate::Result<SchemaModel>
where
    B: IntrospectionBackend,
    P: PoolProvider<B>,
{
    B::introspect_schema(provider.pool()).await
}

#[cfg(feature = "sqlite")]
pub async fn introspect_sqlite_schema(
    provider: &impl PoolProvider<super::SqliteBackend>,
) -> crate::Result<SchemaModel> {
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
        let mut columns = column_rows
            .into_iter()
            .map(|row| {
                let name: String = row.try_get("name")?;
                let sql_type: String = row.try_get("type")?;
                let nullable = row.try_get::<i64, _>("notnull")? == 0;
                let default = row
                    .try_get::<Option<String>, _>("dflt_value")?
                    .map(|value| super::dialect::canonicalize_column_default_expression(&value));
                let is_primary_key = row.try_get::<i64, _>("pk")? > 0;
                Ok(ColumnModel {
                    name,
                    sql_type,
                    spatial: None,
                    nullable,
                    is_primary_key,
                    // Populated below from UNIQUE constraint autoindexes.
                    is_unique: false,
                    default,
                })
            })
            .collect::<crate::Result<Vec<_>>>()?;
        let primary_keys = columns
            .iter()
            .filter(|column| column.is_primary_key)
            .map(|column| column.name.clone())
            .collect::<Vec<_>>();
        let primary_key = primary_keys
            .first()
            .cloned()
            .unwrap_or_else(|| "id".to_string());

        let pragma_index_list = format!("PRAGMA index_list({})", table_name);
        let index_rows = sqlx::query(&pragma_index_list).fetch_all(pool).await?;
        let mut indexes = Vec::new();
        let mut composite_unique_indexes = Vec::new();
        for row in index_rows {
            let index_name: String = row.try_get("name")?;
            let unique = row.try_get::<i64, _>("unique")? != 0;
            // SQLite 3.16+: origin is "c" (CREATE INDEX), "u" (UNIQUE constraint),
            // or "pk" (PRIMARY KEY). Older builds omit the column; treat missing as "c".
            let origin = row
                .try_get::<String, _>("origin")
                .unwrap_or_else(|_| "c".to_string());

            let pragma_index_info = format!("PRAGMA index_info({})", index_name);
            let index_info_rows = sqlx::query(&pragma_index_info).fetch_all(pool).await?;
            let mut column_names = index_info_rows
                .into_iter()
                .map(|index_row| {
                    // seqno order is reliable for composite UNIQUE constraints.
                    let seqno: i64 = index_row.try_get("seqno").unwrap_or(0);
                    let name: String = index_row.try_get("name")?;
                    Ok((seqno, name))
                })
                .collect::<Result<Vec<_>, sqlx::Error>>()?;
            column_names.sort_by_key(|(seqno, _)| *seqno);
            let column_names = column_names
                .into_iter()
                .map(|(_, name)| name)
                .collect::<Vec<_>>();

            // Inline UNIQUE / PRIMARY KEY constraints become sqlite_autoindex_*
            // entries. Named CREATE INDEX / CREATE UNIQUE INDEX keep their names.
            if index_name.starts_with("sqlite_autoindex_") || origin == "u" || origin == "pk" {
                // Map UNIQUE constraints back onto column / composite metadata so
                // #[unique] fields replan as no-ops after file reopen. PRIMARY KEY
                // autoindexes (origin pk) do not set is_unique.
                if unique && origin == "u" {
                    if column_names.len() == 1 {
                        if let Some(column) = columns
                            .iter_mut()
                            .find(|column| column.name == column_names[0])
                        {
                            // Keep primary-key identity separate; generated
                            // schemas mark PK via is_primary_key, not is_unique.
                            if !column.is_primary_key {
                                column.is_unique = true;
                            }
                        }
                    } else if column_names.len() > 1 {
                        composite_unique_indexes.push(column_names);
                    }
                }
                continue;
            }

            let leaked_name: &'static str = Box::leak(index_name.into_boxed_str());
            let leaked_columns: &'static [&'static str] = Box::leak(
                column_names
                    .into_iter()
                    .map(|column| Box::leak(column.into_boxed_str()) as &'static str)
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            );
            indexes.push(super::core::IndexDef {
                name: leaked_name,
                columns: leaked_columns,
                is_unique: unique,
                method: IndexMethod::Default,
                is_spatial: false,
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
                        "CASCADE" => super::core::DeletePolicy::Cascade,
                        "SET NULL" => super::core::DeletePolicy::SetNull,
                        _ => super::core::DeletePolicy::Restrict,
                    },
                })
            })
            .collect::<crate::Result<Vec<_>>>()?;

        tables.push(TableModel {
            entity_name: table_name.clone(),
            table_name,
            primary_key: primary_key.clone(),
            primary_keys,
            default_sort: primary_key.clone(),
            columns,
            indexes,
            composite_unique_indexes,
            foreign_keys,
            search_indexes: Vec::new(),
        });
    }

    let metadata_exists = sqlx::query(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = '__graphql_orm_search_metadata'",
    )
    .fetch_optional(pool)
    .await?
    .is_some();
    if metadata_exists {
        let rows = sqlx::query(
            "SELECT config_json FROM __graphql_orm_search_metadata ORDER BY entity_name",
        )
        .fetch_all(pool)
        .await?;
        let search_indexes = rows
            .into_iter()
            .filter_map(|row| {
                row.try_get::<String, _>("config_json")
                    .ok()
                    .and_then(|config| parse_search_index_config(&config))
            })
            .collect::<Vec<_>>();
        attach_search_indexes(&mut tables, search_indexes);
    }

    Ok(SchemaModel {
        extensions: Vec::new(),
        tables,
    })
}

#[cfg(feature = "sqlite")]
impl IntrospectionBackend for super::SqliteBackend {
    async fn introspect_schema(pool: &Self::Pool) -> crate::Result<SchemaModel> {
        introspect_sqlite_schema(pool).await
    }
}

#[cfg(feature = "sqlite")]
impl RlsIntrospectionBackend for super::SqliteBackend {}

#[cfg(feature = "postgres")]
fn parse_postgres_geometry_type(sql_type: &str) -> Option<SpatialColumnDef> {
    let trimmed = sql_type.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower == "geometry" {
        return Some(SpatialColumnDef::geometry(SpatialGeometryType::Geometry, 0));
    }
    if !lower.starts_with("geometry(") || !lower.ends_with(')') {
        return None;
    }

    let inner = &trimmed["geometry(".len()..trimmed.len() - 1];
    let mut parts = inner.split(',').map(str::trim);
    let geometry_type = parts
        .next()
        .and_then(SpatialGeometryType::from_sql)
        .unwrap_or(SpatialGeometryType::Geometry);
    let srid = parts
        .next()
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(0);
    Some(SpatialColumnDef::geometry(geometry_type, srid))
}

#[cfg(feature = "postgres")]
pub async fn introspect_postgres_schema(
    provider: &impl PoolProvider<super::PostgresBackend>,
) -> crate::Result<SchemaModel> {
    let pool = provider.pool();
    let schema_name = sqlx::query("SELECT current_schema() AS schema_name")
        .fetch_one(pool)
        .await?
        .try_get::<Option<String>, _>("schema_name")?
        .unwrap_or_else(|| "public".to_string());
    let extension_rows = sqlx::query("SELECT extname FROM pg_extension ORDER BY extname")
        .fetch_all(pool)
        .await?;
    let extensions = extension_rows
        .into_iter()
        .map(|row| row.try_get::<String, _>("extname"))
        .collect::<Result<Vec<_>, _>>()?;

    let table_rows = sqlx::query(
        "SELECT table_name
         FROM information_schema.tables
         WHERE table_schema = $1 AND table_type = 'BASE TABLE'
         ORDER BY table_name",
    )
    .bind(&schema_name)
    .fetch_all(pool)
    .await?;

    let mut tables = Vec::new();
    for row in table_rows {
        let table_name: String = row.try_get("table_name")?;
        if is_internal_graphql_orm_table(&table_name) {
            continue;
        }
        let column_rows = sqlx::query(
            "SELECT a.attname AS column_name,
                    format_type(a.atttypid, a.atttypmod) AS data_type,
                    NOT a.attnotnull AS nullable,
                    pg_get_expr(ad.adbin, ad.adrelid) AS column_default
             FROM pg_attribute a
             JOIN pg_class c ON c.oid = a.attrelid
             JOIN pg_namespace n ON n.oid = c.relnamespace
             LEFT JOIN pg_attrdef ad
               ON ad.adrelid = a.attrelid
              AND ad.adnum = a.attnum
             WHERE n.nspname = $2
               AND c.relname = $1
               AND a.attnum > 0
               AND NOT a.attisdropped
             ORDER BY a.attnum",
        )
        .bind(&table_name)
        .bind(&schema_name)
        .fetch_all(pool)
        .await?;

        let primary_key_rows = sqlx::query(
            "SELECT kcu.column_name
             FROM information_schema.table_constraints tc
             JOIN information_schema.key_column_usage kcu
               ON tc.constraint_name = kcu.constraint_name
              AND tc.table_schema = kcu.table_schema
             WHERE tc.table_schema = $2
               AND tc.table_name = $1
               AND tc.constraint_type = 'PRIMARY KEY'",
        )
        .bind(&table_name)
        .bind(&schema_name)
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
             WHERE tc.table_schema = $2
               AND tc.table_name = $1
               AND tc.constraint_type = 'UNIQUE'",
        )
        .bind(&table_name)
        .bind(&schema_name)
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
                let sql_type: String = row.try_get("data_type")?;
                let spatial = parse_postgres_geometry_type(&sql_type);
                Ok(ColumnModel {
                    is_primary_key: primary_key_columns.iter().any(|column| column == &name),
                    is_unique: unique_columns.contains(&name),
                    name,
                    sql_type,
                    spatial,
                    nullable: row.try_get::<bool, _>("nullable")?,
                    default: row.try_get::<Option<String>, _>("column_default")?,
                })
            })
            .collect::<crate::Result<Vec<_>>>()?;

        let primary_key = primary_key_columns
            .first()
            .cloned()
            .unwrap_or_else(|| "id".to_string());

        let index_rows = sqlx::query(
            "SELECT i.relname AS indexname,
                    ix.indisunique AS is_unique,
                    am.amname AS method,
                    array_remove(array_agg(a.attname ORDER BY cols.ordinality), NULL) AS columns
             FROM pg_class t
             JOIN pg_namespace n ON n.oid = t.relnamespace
             JOIN pg_index ix ON ix.indrelid = t.oid
             JOIN pg_class i ON i.oid = ix.indexrelid
             JOIN pg_am am ON am.oid = i.relam
             LEFT JOIN LATERAL unnest(ix.indkey) WITH ORDINALITY AS cols(attnum, ordinality)
               ON true
             LEFT JOIN pg_attribute a
               ON a.attrelid = t.oid
              AND a.attnum = cols.attnum
             WHERE n.nspname = $2
               AND t.relname = $1
             GROUP BY i.relname, ix.indisunique, am.amname
             ORDER BY i.relname",
        )
        .bind(&table_name)
        .bind(&schema_name)
        .fetch_all(pool)
        .await?;
        let mut indexes = Vec::new();
        for row in index_rows {
            let index_name: String = row.try_get("indexname")?;
            if index_name.ends_with("_pkey") {
                continue;
            }
            let unique: bool = row.try_get("is_unique")?;
            let method_name: String = row.try_get("method")?;
            let method = match method_name.to_ascii_lowercase().as_str() {
                "gist" => IndexMethod::Gist,
                _ => IndexMethod::Default,
            };
            let column_names: Vec<String> = row.try_get("columns")?;
            let is_spatial = method == IndexMethod::Gist
                && column_names.iter().any(|column_name| {
                    columns
                        .iter()
                        .any(|column| column.name == *column_name && column.spatial.is_some())
                });
            let leaked_name: &'static str = Box::leak(index_name.into_boxed_str());
            let leaked_columns: &'static [&'static str] = Box::leak(
                column_names
                    .into_iter()
                    .map(|column| Box::leak(column.into_boxed_str()) as &'static str)
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            );
            indexes.push(super::core::IndexDef {
                name: leaked_name,
                columns: leaked_columns,
                is_unique: unique,
                method,
                is_spatial,
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
              AND ccu.constraint_schema = tc.table_schema
             JOIN information_schema.referential_constraints rc
               ON rc.constraint_name = tc.constraint_name
              AND rc.constraint_schema = tc.table_schema
             WHERE tc.table_schema = $2
               AND tc.table_name = $1
               AND tc.constraint_type = 'FOREIGN KEY'",
        )
        .bind(&table_name)
        .bind(&schema_name)
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
                        "CASCADE" => super::core::DeletePolicy::Cascade,
                        "SET NULL" => super::core::DeletePolicy::SetNull,
                        _ => super::core::DeletePolicy::Restrict,
                    },
                })
            })
            .collect::<crate::Result<Vec<_>>>()?;

        tables.push(TableModel {
            entity_name: table_name.clone(),
            table_name,
            primary_key: primary_key.clone(),
            primary_keys: primary_key_columns.clone(),
            default_sort: primary_key,
            columns,
            indexes,
            composite_unique_indexes: Vec::new(),
            foreign_keys,
            search_indexes: Vec::new(),
        });
    }

    let metadata_exists = sqlx::query(
        "SELECT EXISTS (
            SELECT 1
            FROM pg_class c
            JOIN pg_namespace n ON n.oid = c.relnamespace
            WHERE n.nspname = $1
              AND c.relname = $2
        ) AS exists",
    )
    .bind(&schema_name)
    .bind(super::search_metadata_table_name())
    .fetch_one(pool)
    .await?
    .try_get::<bool, _>("exists")?;
    if metadata_exists {
        let rows = sqlx::query(
            "SELECT config_json::text AS config_json FROM __graphql_orm_search_metadata ORDER BY entity_name",
        )
        .fetch_all(pool)
        .await?;
        let search_indexes = rows
            .into_iter()
            .filter_map(|row| {
                row.try_get::<String, _>("config_json")
                    .ok()
                    .and_then(|config| parse_search_index_config(&config))
            })
            .collect::<Vec<_>>();
        attach_search_indexes(&mut tables, search_indexes);
    }

    Ok(SchemaModel { extensions, tables })
}

#[cfg(feature = "postgres")]
impl IntrospectionBackend for super::PostgresBackend {
    async fn introspect_schema(pool: &Self::Pool) -> crate::Result<SchemaModel> {
        introspect_postgres_schema(pool).await
    }
}

#[cfg(feature = "postgres")]
impl RlsIntrospectionBackend for super::PostgresBackend {
    async fn introspect_rls(pool: &Self::Pool) -> crate::Result<Vec<LiveRlsTable>> {
        let table_rows = sqlx::query(
            "SELECT n.nspname AS schema_name,
                    c.relname AS table_name,
                    c.relrowsecurity AS enabled,
                    c.relforcerowsecurity AS forced
             FROM pg_class c
             JOIN pg_namespace n ON n.oid = c.relnamespace
             WHERE c.relkind IN ('r', 'p')
               AND n.nspname NOT IN ('pg_catalog', 'information_schema')
             ORDER BY n.nspname, c.relname",
        )
        .fetch_all(pool)
        .await?;

        let policy_rows = sqlx::query(
            "SELECT schemaname,
                    tablename,
                    policyname,
                    cmd,
                    qual,
                    with_check
             FROM pg_policies
             ORDER BY schemaname, tablename, policyname",
        )
        .fetch_all(pool)
        .await?;

        let mut policies_by_table: std::collections::BTreeMap<String, Vec<LiveRlsPolicy>> =
            std::collections::BTreeMap::new();
        for row in policy_rows {
            let schema_name: String = row.try_get("schemaname")?;
            let table_name: String = row.try_get("tablename")?;
            let key = if schema_name == "public" {
                table_name.clone()
            } else {
                format!("{schema_name}_{table_name}")
            };
            let cmd: String = row.try_get("cmd")?;
            let operation = match cmd.as_str() {
                "SELECT" => RlsOperation::Select,
                "INSERT" => RlsOperation::Insert,
                "UPDATE" => RlsOperation::Update,
                "DELETE" => RlsOperation::Delete,
                _ => continue,
            };
            policies_by_table
                .entry(key)
                .or_default()
                .push(LiveRlsPolicy {
                    policy_name: row.try_get("policyname")?,
                    operation,
                    using_expression: row.try_get("qual")?,
                    check_expression: row.try_get("with_check")?,
                });
        }

        table_rows
            .into_iter()
            .map(|row| {
                let schema_name: String = row.try_get("schema_name")?;
                let table_name: String = row.try_get("table_name")?;
                let key = if schema_name == "public" {
                    table_name.clone()
                } else {
                    format!("{schema_name}_{table_name}")
                };
                Ok(LiveRlsTable {
                    table_name: key.clone(),
                    enabled: row.try_get("enabled")?,
                    forced: row.try_get("forced")?,
                    policies: policies_by_table.remove(&key).unwrap_or_default(),
                })
            })
            .collect()
    }
}

#[cfg(feature = "mssql")]
pub async fn introspect_mssql_schema(
    provider: &impl PoolProvider<super::MssqlBackend>,
) -> crate::Result<SchemaModel> {
    let pool = provider.pool();
    let table_rows = super::MssqlBackend::fetch_rows(
        pool,
        "SELECT TABLE_SCHEMA AS table_schema, TABLE_NAME AS table_name
         FROM INFORMATION_SCHEMA.TABLES
         WHERE TABLE_TYPE = 'BASE TABLE'
           AND TABLE_SCHEMA NOT IN ('sys', 'INFORMATION_SCHEMA')
         ORDER BY TABLE_SCHEMA, TABLE_NAME",
        &[],
    )
    .await?;

    let mut tables = Vec::new();
    for row in table_rows {
        let schema_name = super::MssqlBackend::try_get_string(&row, "table_schema")?;
        let raw_table_name = super::MssqlBackend::try_get_string(&row, "table_name")?;
        let table_path = format!("{schema_name}.{raw_table_name}");
        if is_internal_graphql_orm_table(&raw_table_name) {
            continue;
        }

        let column_rows = super::MssqlBackend::fetch_rows(
            pool,
            "SELECT COLUMN_NAME AS column_name,
                    DATA_TYPE AS data_type,
                    IS_NULLABLE AS is_nullable,
                    COLUMN_DEFAULT AS column_default
             FROM INFORMATION_SCHEMA.COLUMNS
             WHERE TABLE_SCHEMA = @P1 AND TABLE_NAME = @P2
             ORDER BY ORDINAL_POSITION",
            &[
                SqlValue::String(schema_name.clone()),
                SqlValue::String(raw_table_name.clone()),
            ],
        )
        .await?;

        let primary_key_rows = super::MssqlBackend::fetch_rows(
            pool,
            "SELECT kcu.COLUMN_NAME AS column_name
             FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS tc
             JOIN INFORMATION_SCHEMA.KEY_COLUMN_USAGE kcu
               ON tc.CONSTRAINT_NAME = kcu.CONSTRAINT_NAME
              AND tc.TABLE_SCHEMA = kcu.TABLE_SCHEMA
              AND tc.TABLE_NAME = kcu.TABLE_NAME
             WHERE tc.TABLE_SCHEMA = @P1
               AND tc.TABLE_NAME = @P2
               AND tc.CONSTRAINT_TYPE = 'PRIMARY KEY'
             ORDER BY kcu.ORDINAL_POSITION",
            &[
                SqlValue::String(schema_name.clone()),
                SqlValue::String(raw_table_name.clone()),
            ],
        )
        .await?;
        let primary_keys = primary_key_rows
            .into_iter()
            .map(|row| {
                super::MssqlBackend::try_get_string(&row, "column_name")
                    .map(|column| DatabaseBackend::Mssql.quote_identifier(&column))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let primary_key = primary_keys
            .first()
            .cloned()
            .unwrap_or_else(|| DatabaseBackend::Mssql.quote_identifier("id"));

        let unique_rows = super::MssqlBackend::fetch_rows(
            pool,
            "SELECT kcu.COLUMN_NAME AS column_name
             FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS tc
             JOIN INFORMATION_SCHEMA.KEY_COLUMN_USAGE kcu
               ON tc.CONSTRAINT_NAME = kcu.CONSTRAINT_NAME
              AND tc.TABLE_SCHEMA = kcu.TABLE_SCHEMA
              AND tc.TABLE_NAME = kcu.TABLE_NAME
             WHERE tc.TABLE_SCHEMA = @P1
               AND tc.TABLE_NAME = @P2
               AND tc.CONSTRAINT_TYPE = 'UNIQUE'",
            &[
                SqlValue::String(schema_name.clone()),
                SqlValue::String(raw_table_name.clone()),
            ],
        )
        .await?;
        let unique_columns = unique_rows
            .into_iter()
            .map(|row| {
                super::MssqlBackend::try_get_string(&row, "column_name")
                    .map(|column| DatabaseBackend::Mssql.quote_identifier(&column))
            })
            .collect::<Result<std::collections::HashSet<_>, _>>()?;

        let columns = column_rows
            .into_iter()
            .map(|row| {
                let raw_name = super::MssqlBackend::try_get_string(&row, "column_name")?;
                let name = DatabaseBackend::Mssql.quote_identifier(&raw_name);
                Ok(ColumnModel {
                    is_primary_key: primary_keys.iter().any(|column| column == &name),
                    is_unique: unique_columns.contains(&name),
                    name,
                    sql_type: super::MssqlBackend::try_get_string(&row, "data_type")?,
                    spatial: None,
                    nullable: super::MssqlBackend::try_get_string(&row, "is_nullable")? == "YES",
                    default: row.try_get::<Option<String>, _>("column_default")?,
                })
            })
            .collect::<crate::Result<Vec<_>>>()?;

        tables.push(TableModel {
            entity_name: raw_table_name.clone(),
            table_name: DatabaseBackend::Mssql.quote_identifier_path(&table_path),
            primary_key: primary_key.clone(),
            primary_keys,
            default_sort: format!("{primary_key} ASC"),
            columns,
            indexes: Vec::new(),
            composite_unique_indexes: Vec::new(),
            foreign_keys: Vec::new(),
            search_indexes: Vec::new(),
        });
    }

    Ok(SchemaModel {
        extensions: Vec::new(),
        tables,
    })
}

#[cfg(feature = "mssql")]
impl IntrospectionBackend for super::MssqlBackend {
    async fn introspect_schema(pool: &Self::Pool) -> crate::Result<SchemaModel> {
        introspect_mssql_schema(pool).await
    }
}

#[cfg(feature = "mssql")]
impl RlsIntrospectionBackend for super::MssqlBackend {}
