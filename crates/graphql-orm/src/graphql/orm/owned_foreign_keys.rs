//! Capability checks supplement the legacy physical model without changing its public layout.
//! Never let lossy legacy FK introspection authorize owned replacement or certify mutations.
#[cfg(any(feature = "sqlite", feature = "postgres"))]
use super::runtime_migration::{
    ManagedTableSet, RuntimeMigrationDiagnostic, RuntimeMigrationDiagnosticCode,
    RuntimeMigrationDiagnostics, RuntimeMigrationError,
};

#[cfg(any(feature = "sqlite", feature = "postgres"))]
fn unsupported(table: String, subject: String) -> RuntimeMigrationError {
    RuntimeMigrationDiagnostics(vec![RuntimeMigrationDiagnostic {
        code: RuntimeMigrationDiagnosticCode::UnsupportedForeignKey,
        // This is a physical catalog source, potentially absent from the runtime schema.
        collection: None,
        subject: Some(format!("{table}.{subject}")),
    }])
    .into()
}

#[cfg(feature = "sqlite")]
pub(super) async fn validate_sqlite(
    connection: &mut sqlx::SqliteConnection,
    ownership: &ManagedTableSet,
) -> Result<(), RuntimeMigrationError> {
    use sqlx::Row;
    // No reserved-name filter: system/ORM tables can carry incoming modifying actions.
    let rows = sqlx::query(
        "SELECT m.name, m.sql, f.id, f.\"from\" AS source_column,
                f.\"table\" AS target_table, f.on_delete, f.on_update
         FROM sqlite_master m JOIN pragma_foreign_key_list(m.name) f
         WHERE m.type = 'table' ORDER BY m.name, f.id, f.seq",
    )
    .fetch_all(&mut *connection)
    .await?;
    for row in rows {
        let source: String = row.try_get("name")?;
        let target: String = row.try_get("target_table")?;
        let touches_owned = ownership.tables().any(|table| {
            table.eq_ignore_ascii_case(&source) || table.eq_ignore_ascii_case(&target)
        });
        if !touches_owned {
            continue;
        }
        let delete: String = row.try_get("on_delete")?;
        let update: String = row.try_get("on_update")?;
        let sql: String = row.try_get("sql")?;
        // NO ACTION deletion is also distinct from the model's RESTRICT timing.
        // Update NO ACTION is what canonical CREATE TABLE emits by omission.
        if !matches!(delete.as_str(), "RESTRICT" | "CASCADE" | "SET NULL")
            || update != "NO ACTION"
            || ownership.tables().any(|table| {
                (table.eq_ignore_ascii_case(&source) && table != source)
                    || (table.eq_ignore_ascii_case(&target) && table != target)
            })
            || has_deferrable_clause(&sql)
        {
            return Err(unsupported(source, row.try_get("source_column")?));
        }
    }
    Ok(())
}

/// Extract only unquoted SQL words. Literals, quoted identifiers and comments cannot
/// masquerade as deferral clauses. SQLite doesn't expose deferral in its FK PRAGMA.
#[cfg(feature = "sqlite")]
fn has_deferrable_clause(sql: &str) -> bool {
    let mut chars = sql.chars().peekable();
    let mut previous = String::new();
    while let Some(character) = chars.next() {
        match character {
            '\'' | '"' | '`' | '[' => {
                let close = if character == '[' { ']' } else { character };
                while let Some(next) = chars.next() {
                    if next == close {
                        if close != ']' && chars.peek() == Some(&close) {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                }
                previous.clear();
            }
            '-' if chars.peek() == Some(&'-') => {
                chars.next();
                for next in chars.by_ref() {
                    if next == '\n' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                while let Some(next) = chars.next() {
                    if next == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        break;
                    }
                }
            }
            first if first.is_ascii_alphabetic() || first == '_' => {
                let mut word = first.to_string();
                while let Some(next) = chars.next_if(|c| c.is_ascii_alphanumeric() || *c == '_') {
                    word.push(next);
                }
                if word.eq_ignore_ascii_case("deferrable") && !previous.eq_ignore_ascii_case("not")
                {
                    return true;
                }
                previous = word;
            }
            c if !c.is_whitespace() => previous.clear(),
            _ => {}
        }
    }
    false
}

#[cfg(feature = "postgres")]
pub(super) async fn validate_postgres(
    connection: &mut sqlx::PgConnection,
    ownership: &ManagedTableSet,
) -> Result<(), RuntimeMigrationError> {
    // pg_catalog includes incoming constraints from other schemas and reserved/system
    // sources. information_schema may omit constraints and does not expose every action.
    let tables: Vec<&str> = ownership.tables().collect();
    let unsupported: Option<(String, String)> = sqlx::query_as(
        "SELECT sn.nspname || '.' || s.relname, con.conname
         FROM pg_constraint con
         JOIN pg_class s ON s.oid = con.conrelid
         JOIN pg_namespace sn ON sn.oid = s.relnamespace
         JOIN pg_class t ON t.oid = con.confrelid
         JOIN pg_namespace tn ON tn.oid = t.relnamespace
         WHERE con.contype = 'f'
           AND ((sn.nspname = current_schema() AND s.relname = ANY($1))
             OR (tn.nspname = current_schema() AND t.relname = ANY($1)))
           AND (con.confdeltype NOT IN ('r', 'c', 'n') OR con.confupdtype <> 'a'
                OR con.condeferrable OR con.condeferred OR con.confmatchtype <> 's'
                OR NOT con.convalidated
                OR COALESCE(to_jsonb(con)->>'confdelsetcols', '') <> ''
                OR sn.nspname <> current_schema() OR tn.nspname <> current_schema())
         ORDER BY sn.nspname, s.relname, con.conname LIMIT 1",
    )
    .bind(tables)
    .fetch_optional(&mut *connection)
    .await?;
    if let Some((table, constraint)) = unsupported {
        return Err(self::unsupported(table, constraint));
    }
    Ok(())
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::has_deferrable_clause;
    #[test]
    fn sqlite_deferral_scan_ignores_literals_identifiers_and_comments() {
        assert!(has_deferrable_clause(
            "REFERENCES t(id) DEFERRABLE INITIALLY DEFERRED"
        ));
        assert!(has_deferrable_clause(
            "REFERENCES t(id) deferrable initially immediate"
        ));
        assert!(!has_deferrable_clause(
            "REFERENCES t(id) NOT /* comment */ DEFERRABLE"
        ));
        assert!(!has_deferrable_clause(
            "'DEFERRABLE' \"DEFERRABLE\" [DEFERRABLE] `DEFERRABLE` -- DEFERRABLE\n /* DEFERRABLE */"
        ));
        assert!(!has_deferrable_clause(
            "'can''t DEFERRABLE' \"a\"\"DEFERRABLE\""
        ));
    }
}
