//! Preflight the exact native ALTER against an empty copy of the main schema.
//! SQLite resolves SQL dependencies in that copy before the live transaction
//! executes either the ALTER or migration-history DDL.
use super::{DatabaseBackend, Token, clauses, punctuation, signature, tokens};
use crate::sqlx::{self, Connection};

#[derive(Debug, PartialEq, Eq)]
struct Object {
    kind: String,
    name: String,
    tbl_name: String,
    rootpage: i64,
    sql: Option<String>,
}

async fn catalog(conn: &mut sqlx::SqliteConnection) -> Result<Vec<Object>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, String, String, i64, Option<String>)>(
        "SELECT type, name, tbl_name, rootpage, sql FROM main.sqlite_schema ORDER BY rowid",
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(kind, name, tbl_name, rootpage, sql)| Object {
            kind,
            name,
            tbl_name,
            rootpage,
            sql,
        })
        .collect())
}

fn refused(dependency: impl std::fmt::Display) -> sqlx::Error {
    sqlx::Error::Protocol(format!(
        "UnsupportedCapability: SQLite column drop: {dependency}"
    ))
}

fn quoted(name: &str) -> String {
    DatabaseBackend::Sqlite.delimit_raw_identifier(name)
}

fn is_identifier(sql: &str, token: &Token, name: &str) -> bool {
    token
        .ident
        .as_deref()
        .is_some_and(|ident| ident.eq_ignore_ascii_case(name))
        || sql[token.span.clone()]
            .strip_prefix('\'')
            .and_then(|sql| sql.strip_suffix('\''))
            .is_some_and(|ident| ident.replace("''", "'").eq_ignore_ascii_case(name))
}

// sqlite_schema records are parsed by SQLite when loaded, but loading the
// catalog only prepares the first statement. Never execute a trailing statement
// from a hand-edited catalog on the probe connection.
fn single_declaration(sql: &str, ts: &[Token], kind: &str) -> bool {
    let modifiers = ts
        .iter()
        .skip(1)
        .take_while(|t| t.keyword("unique") || t.keyword("virtual"))
        .count();
    if !ts.first().is_some_and(|t| t.keyword("create"))
        || !ts.get(1 + modifiers).is_some_and(|t| t.keyword(kind))
    {
        return false;
    }
    for (index, token) in ts.iter().enumerate() {
        if punctuation(sql, token, ";") {
            let rest = &ts[index + 1..];
            if kind != "trigger" || rest.is_empty() {
                return rest.is_empty();
            }
            if rest[0].keyword("end") {
                return rest.len() == 1 || (rest.len() == 2 && punctuation(sql, &rest[1], ";"));
            }
        }
    }
    true
}

async fn foreign_key_dependency(
    conn: &mut sqlx::SqliteConnection,
    table: &str,
    column: &str,
) -> Result<(), sqlx::Error> {
    let dependency: Option<(String, i64, String)> = sqlx::query_as(
        r#"SELECT m.name, f.id, f."from"
           FROM main.sqlite_schema m JOIN pragma_foreign_key_list(m.name, 'main') f
           WHERE m.type = 'table' AND (
                (m.name = ?1 COLLATE NOCASE AND f."from" = ?2 COLLATE NOCASE)
                OR (f."table" = ?1 COLLATE NOCASE AND f."to" = ?2 COLLATE NOCASE)
           ) LIMIT 1"#,
    )
    .bind(table)
    .bind(column)
    .fetch_optional(conn)
    .await?;
    if let Some((source_table, id, source_column)) = dependency {
        return Err(refused(format!(
            "foreign key {id} on {source_table}.{source_column} depends on {table}.{column}"
        )));
    }
    Ok(())
}

fn check_implicit_dependencies(
    object: &Object,
    sql: &str,
    ts: &[Token],
    table: &str,
    column: &str,
) -> Result<(), sqlx::Error> {
    let names_table = ts.iter().any(|token| is_identifier(sql, token, table));
    if !names_table && object.tbl_name != table {
        return Ok(());
    }
    // Native reparsing can silently change the meaning of a wildcard or a
    // NATURAL JOIN. Refuse those implicit references to the altered table.
    let wildcard = ts.windows(2).any(|pair| {
        punctuation(sql, &pair[1], "*")
            && (punctuation(sql, &pair[0], ".")
                || punctuation(sql, &pair[0], ",")
                || ["select", "distinct", "all"]
                    .iter()
                    .any(|kw| pair[0].keyword(kw)))
    });
    if wildcard || ts.iter().any(|token| token.keyword("natural")) {
        return Err(refused(format!(
            "{} {} may depend on {table}.{column}",
            object.kind, object.name
        )));
    }
    Ok(())
}

// DROP can silently rebind a reference to an outer query or ignore a trigger's
// UPDATE OF column. RENAME rewrites the original bindings, exposing both.
async fn check_bound_dependencies(
    probe: &mut sqlx::SqliteConnection,
    before: &[Object],
    table: &str,
    column: &str,
) -> Result<(), sqlx::Error> {
    if !before
        .iter()
        .any(|object| matches!(object.kind.as_str(), "view" | "trigger"))
    {
        return Ok(());
    }
    let statement = format!(
        "ALTER TABLE main.{} RENAME COLUMN {} TO {}",
        quoted(table),
        quoted(column),
        quoted(&format!(
            "__graphql_orm_drop_probe_{}",
            uuid::Uuid::new_v4().simple()
        )),
    );
    let mut rename = probe.begin().await?;
    sqlx::query(&statement)
        .execute(&mut *rename)
        .await
        .map_err(|error| {
            refused(format!(
                "cannot verify references to {table}.{column}: {error}"
            ))
        })?;
    let after = catalog(&mut rename).await?;
    for (prior, current) in before.iter().zip(&after) {
        if matches!(prior.kind.as_str(), "view" | "trigger") && prior != current {
            return Err(refused(format!(
                "{} {} depends on {table}.{column}",
                prior.kind, prior.name
            )));
        }
    }
    rename.rollback().await?;
    Ok(())
}

async fn view_columns(
    conn: &mut sqlx::SqliteConnection,
    name: &str,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT name FROM pragma_table_info(?, 'main') ORDER BY cid")
        .bind(name)
        .fetch_all(conn)
        .await
        .map_err(|error| refused(format!("view {name}: {error}")))
}

// Native ALTER resolves references in trigger expressions, but doesn't
// compile trigger DML. EXPLAIN also checks INSERT arity and target columns.
async fn validate_trigger(
    conn: &mut sqlx::SqliteConnection,
    object: &Object,
    sql: &str,
) -> Result<(), sqlx::Error> {
    let ts = tokens(sql)?;
    let event = ts
        .iter()
        .skip(3)
        .find(|token| {
            ["insert", "delete", "update"]
                .iter()
                .any(|keyword| token.keyword(keyword))
        })
        .ok_or_else(|| refused(format!("cannot identify event for trigger {}", object.name)))?;
    let subject = quoted(&object.tbl_name);
    let explain = if event.keyword("insert") {
        format!("EXPLAIN INSERT INTO main.{subject} DEFAULT VALUES")
    } else if event.keyword("delete") {
        format!("EXPLAIN DELETE FROM main.{subject}")
    } else {
        let columns: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM pragma_table_xinfo(?, 'main') WHERE hidden = 0 ORDER BY cid",
        )
        .bind(&object.tbl_name)
        .fetch_all(&mut *conn)
        .await?;
        let assignments = columns
            .iter()
            .map(|name| {
                let name = quoted(name);
                format!("{name} = {name}")
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("EXPLAIN UPDATE main.{subject} SET {assignments}")
    };
    sqlx::query(&explain)
        .fetch_optional(conn)
        .await
        .map_err(|error| refused(format!("trigger {}: {error}", object.name)))?;
    Ok(())
}

// SQLite names the table for failures in CHECK and generated expressions.
// Add the declaration names when the native error comes from that table.
fn table_dependencies(clauses: &[String], column: &str) -> Vec<String> {
    clauses
        .iter()
        .filter_map(|clause| {
            let ts = tokens(clause).ok()?;
            if !ts.iter().any(|token| {
                token
                    .ident
                    .as_deref()
                    .is_some_and(|name| name.eq_ignore_ascii_case(column))
            }) {
                return None;
            }
            if let Some(check) = ts.iter().position(|token| token.keyword("check")) {
                return Some(if check >= 2 && ts[check - 2].keyword("constraint") {
                    format!("CHECK {}", &clause[ts[check - 1].span.clone()])
                } else {
                    "CHECK constraint".into()
                });
            }
            ts.windows(2)
                .any(|pair| pair[0].keyword("as") && punctuation(clause, &pair[1], "("))
                .then(|| format!("generated column {}", &clause[ts[0].span.clone()]))
        })
        .collect()
}

pub(crate) struct PreparedDrop {
    table: String,
    sql_after: String,
}

pub(crate) async fn prepare(
    conn: &mut sqlx::SqliteConnection,
    table: &str,
    column: &str,
    statement: &str,
) -> Result<PreparedDrop, sqlx::Error> {
    for pragma in ["writable_schema", "legacy_alter_table"] {
        let enabled: i64 = sqlx::query_scalar(&format!("PRAGMA {pragma}"))
            .fetch_one(&mut *conn)
            .await?;
        if enabled != 0 {
            return Err(refused(format!("requires {pragma} to be off")));
        }
    }
    if let Some((kind, name)) =
        sqlx::query_as::<_, (String, String)>("SELECT type, name FROM temp.sqlite_schema LIMIT 1")
            .fetch_optional(&mut *conn)
            .await?
    {
        return Err(refused(format!("cannot verify TEMP {kind} {name}")));
    }

    foreign_key_dependency(conn, table, column).await?;
    let objects = catalog(conn).await?;
    let table_sql = objects
        .iter()
        .find(|object| object.kind == "table" && object.name == table)
        .and_then(|object| object.sql.as_deref())
        .ok_or_else(|| refused(format!("table {table} has no CREATE TABLE declaration")))?;
    let (mut retained, tail) =
        clauses(table_sql).map_err(|_| refused(format!("cannot verify table {table}")))?;
    let index = retained
        .iter()
        .position(|part| {
            tokens(part)
                .ok()
                .and_then(|ts| ts.into_iter().next())
                .is_some_and(|token| is_identifier(part, &token, column))
        })
        .ok_or_else(|| refused(format!("cannot find declaration for {table}.{column}")))?;
    let removed = retained.remove(index);
    if tokens(&removed)?.iter().any(|token| token.keyword("check")) {
        return Err(refused(format!("CHECK on {table}.{column}")));
    }

    let shadow_tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM pragma_table_list WHERE schema = 'main' AND type = 'shadow'",
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut probe = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(false),
    )
    .await?;
    let mut views = Vec::new();
    let mut triggers = Vec::new();
    for object in &objects {
        let Some(sql) = &object.sql else { continue };
        // SQLite and the built-in FTS5 module recreate their own objects.
        if object
            .name
            .as_bytes()
            .get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"sqlite_"))
            || (object.kind == "table" && shadow_tables.contains(&object.name))
        {
            continue;
        }
        let ts = tokens(sql)
            .map_err(|_| refused(format!("cannot read {} {}", object.kind, object.name)))?;
        if !single_declaration(sql, &ts, &object.kind) {
            return Err(refused(format!(
                "cannot replay {} {}",
                object.kind, object.name
            )));
        }
        if ts.get(1).is_some_and(|token| token.keyword("virtual"))
            && !ts
                .windows(2)
                .find(|ts| ts[0].keyword("using"))
                .is_some_and(|ts| ts[1].keyword("fts5"))
        {
            return Err(refused(format!(
                "cannot verify virtual table {}",
                object.name
            )));
        }
        if ts.get(1).is_some_and(|token| token.keyword("virtual"))
            && ts.iter().any(|token| is_identifier(sql, token, table))
        {
            return Err(refused(format!(
                "virtual table {} may depend on {table}",
                object.name
            )));
        }
        if matches!(object.kind.as_str(), "view" | "trigger") {
            check_implicit_dependencies(object, sql, &ts, table, column)?;
        }
        sqlx::query(sql)
            .persistent(false)
            .execute(&mut probe)
            .await
            .map_err(|error| refused(format!("{} {}: {error}", object.kind, object.name)))?;
        if object.kind == "trigger" {
            triggers.push((object, sql));
        }
    }
    for object in objects.iter().filter(|object| object.kind == "view") {
        views.push((&object.name, view_columns(&mut probe, &object.name).await?));
    }
    let mut expected = catalog(&mut probe).await?;
    check_bound_dependencies(&mut probe, &expected, table, column).await?;
    sqlx::query(statement)
        .execute(&mut probe)
        .await
        .map_err(|error| {
            let message = error
                .as_database_error()
                .map_or("", |error| error.message());
            if message.starts_with(&format!("error in table {table} after drop column")) {
                let dependencies = table_dependencies(&retained, column);
                if !dependencies.is_empty() {
                    return refused(format!("{table}: {}: {error}", dependencies.join(", ")));
                }
            }
            refused(format!("{table}.{column}: {error}"))
        })?;

    let actual = catalog(&mut probe).await?;
    let changed = actual
        .iter()
        .find(|object| object.kind == "table" && object.name == table)
        .and_then(|object| object.sql.as_ref())
        .ok_or_else(|| refused(format!("ALTER removed table {table}")))?;
    let (actual_clauses, actual_tail) = clauses(changed)?;
    let signatures = |parts: &[String]| {
        parts
            .iter()
            .map(|part| signature(part))
            .collect::<Result<Vec<_>, _>>()
    };
    if signatures(&retained)? != signatures(&actual_clauses)?
        || signature(&tail)? != signature(&actual_tail)?
    {
        return Err(refused(format!(
            "ALTER would change a retained declaration on {table}"
        )));
    }
    verify_catalog(&mut expected, &actual, table, changed)?;
    for (name, before) in views {
        if view_columns(&mut probe, name).await? != before {
            return Err(refused(format!("view {name} depends on {table}.{column}")));
        }
    }
    for (object, sql) in triggers {
        validate_trigger(&mut probe, object, sql).await?;
    }

    Ok(PreparedDrop {
        table: table.into(),
        sql_after: changed.clone(),
    })
}

fn verify_catalog(
    before: &mut [Object],
    after: &[Object],
    table: &str,
    sql_after: &str,
) -> Result<(), sqlx::Error> {
    for (prior, current) in before.iter_mut().zip(after) {
        if prior.kind == "table" && prior.name == table {
            prior.sql = Some(sql_after.into());
        }
        if prior != current {
            return Err(refused(format!(
                "ALTER changed {} {}",
                prior.kind, prior.name
            )));
        }
    }
    if before.len() != after.len() {
        return Err(refused(format!(
            "ALTER changed the object count for {table}"
        )));
    }
    Ok(())
}

impl PreparedDrop {
    pub(crate) async fn apply(
        &self,
        conn: &mut sqlx::SqliteConnection,
        statement: &str,
    ) -> Result<(), sqlx::Error> {
        let mut before = catalog(conn).await?;
        sqlx::query(statement).execute(&mut *conn).await?;
        verify_catalog(
            &mut before,
            &catalog(conn).await?,
            &self.table,
            &self.sql_after,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn preflight_refuses_each_retained_dependency() {
        for (suffix, other_ddl, dependency) in [
            (
                "",
                "CREATE INDEX retained_index ON parent(retired)",
                "index retained_index",
            ),
            (
                "",
                "CREATE INDEX retained_partial ON parent(label) WHERE retired IS NULL",
                "index retained_partial",
            ),
            (
                ", CONSTRAINT kept_check CHECK (retired IS NULL)",
                "",
                "CHECK kept_check",
            ),
            (" CHECK (retired IS NULL)", "", "CHECK on parent.retired"),
            (
                " REFERENCES parent(id)",
                "",
                "foreign key 0 on parent.retired",
            ),
            (
                " UNIQUE",
                "CREATE TABLE child (link REFERENCES parent(retired))",
                "foreign key 0 on child.link",
            ),
            (
                "",
                "CREATE VIEW retained_view AS SELECT retired FROM parent",
                "view retained_view",
            ),
            (
                "",
                "CREATE VIEW retained_view AS SELECT * FROM parent",
                "view retained_view",
            ),
            (
                "",
                "CREATE TABLE outer_table (retired TEXT);
                 CREATE VIEW retained_view AS SELECT o.retired FROM outer_table o
                     WHERE EXISTS (SELECT retired FROM parent)",
                "view retained_view",
            ),
            (
                "",
                "CREATE TRIGGER retained_trigger AFTER UPDATE ON parent BEGIN SELECT OLD.retired; END",
                "trigger retained_trigger",
            ),
            (
                "",
                "CREATE TRIGGER retained_trigger AFTER UPDATE OF 'retired' ON parent BEGIN SELECT 1; END",
                "trigger retained_trigger",
            ),
            (
                "",
                "CREATE TRIGGER retained_trigger AFTER UPDATE ON parent BEGIN INSERT INTO parent VALUES (1, 2, 3); END",
                "trigger retained_trigger",
            ),
            (
                ", derived TEXT GENERATED ALWAYS AS (retired)",
                "",
                "generated column derived",
            ),
            (" UNIQUE", "", "UNIQUE"),
        ] {
            let mut connection = sqlx::SqliteConnection::connect("sqlite::memory:")
                .await
                .unwrap();
            sqlx::raw_sql(&format!(
                "CREATE TABLE parent (id INTEGER PRIMARY KEY, label TEXT, retired TEXT{suffix});
                 {other_ddl};"
            ))
            .execute(&mut connection)
            .await
            .unwrap();
            let before = catalog(&mut connection).await.unwrap();
            let error = prepare(
                &mut connection,
                "parent",
                "retired",
                "ALTER TABLE main.parent DROP COLUMN retired",
            )
            .await
            .err()
            .unwrap_or_else(|| panic!("did not refuse {dependency}"));
            assert!(error.to_string().contains(dependency), "{error}");
            assert_eq!(before, catalog(&mut connection).await.unwrap());
        }
    }
}
