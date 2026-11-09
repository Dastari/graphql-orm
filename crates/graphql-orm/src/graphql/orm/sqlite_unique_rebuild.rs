//! Conservative live-DDL rebuilds. No model-only DDL fallback.
use super::{DatabaseBackend, SQLITE_UNIQUE_REBUILD_PREFIX, SqlDialect, unique_identifier};
use crate::sqlx::{self, Row};
use std::collections::BTreeSet;
use std::ops::Range;

type Keys = Vec<Vec<String>>;
pub(crate) struct Directive {
    pub(crate) table: String,
    before: Keys,
    after: Keys,
    mixed: bool,
    ddl: Option<(String, String)>,
}
pub(crate) fn unsupported() -> sqlx::Error {
    sqlx::Error::Protocol(
        "UnsupportedCapability: SQLite live rebuild cannot preserve this physical schema".into(),
    )
}
pub(crate) fn directive(sql: &str) -> Option<Result<Directive, sqlx::Error>> {
    let sql = sql.trim();
    let sql = sql.strip_suffix(';').unwrap_or(sql).trim_end();
    if let Some(body) = sql.strip_prefix("SELECT graphql_orm_rebuild_table('") {
        return Some((|| {
            let body = body.strip_suffix("')").ok_or_else(unsupported)?;
            let (table, before, after): (String, String, String) =
                serde_json::from_str(&body.replace("''", "'")).map_err(|_| unsupported())?;
            Ok(Directive {
                table,
                before: vec![],
                after: vec![],
                mixed: false,
                ddl: Some((before, after)),
            })
        })());
    }
    let body = sql.strip_prefix(SQLITE_UNIQUE_REBUILD_PREFIX)?;
    Some((|| {
        let body = body.strip_suffix("')").ok_or_else(unsupported)?;
        let (table, before, after, mixed) =
            serde_json::from_str(&body.replace("''", "'")).map_err(|_| unsupported())?;
        Ok(Directive {
            table,
            before,
            after,
            mixed,
            ddl: None,
        })
    })())
}
#[derive(Clone)]
struct Token {
    span: Range<usize>,
    ident: Option<String>,
    quoted: bool,
}
impl Token {
    fn keyword(&self, word: &str) -> bool {
        !self.quoted
            && self
                .ident
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(word))
    }
}
// Token offsets retain the original bytes, including comments and expressions.
// Only identifiers and balanced delimiter boundaries are interpreted.
fn tokens(sql: &str) -> Result<Vec<Token>, sqlx::Error> {
    let b = sql.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b[i..].starts_with(b"--") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b[i..].starts_with(b"/*") {
            i += 2;
            while i + 1 < b.len() && &b[i..i + 2] != b"*/" {
                i += 1;
            }
            if i + 1 >= b.len() {
                return Err(unsupported());
            }
            i += 2;
            continue;
        }
        let start = i;
        let mut quoted = false;
        let mut ident = None;
        if matches!(b[i], b'\'' | b'"' | b'`' | b'[') {
            let open = b[i];
            let close = if open == b'[' { b']' } else { open };
            i += 1;
            loop {
                if i >= b.len() {
                    return Err(unsupported());
                }
                if b[i] == close {
                    i += 1;
                    if close != b']' && i < b.len() && b[i] == close {
                        i += 1;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
            if open != b'\'' {
                ident = Some(unique_identifier(&sql[start..i]).0);
                quoted = true;
            }
        } else if b[i].is_ascii_alphabetic() || b[i] == b'_' || b[i] >= 128 {
            i += 1;
            while i < b.len()
                && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'_' | b'$') || b[i] >= 128)
            {
                i += 1;
            }
            ident = Some(sql[start..i].to_string());
        } else {
            i += 1;
        }
        out.push(Token {
            span: start..i,
            ident,
            quoted,
        });
    }
    Ok(out)
}
fn punctuation(sql: &str, t: &Token, value: &str) -> bool {
    t.ident.is_none() && &sql[t.span.clone()] == value
}
fn key(sql: &str, ts: &[Token]) -> Result<Option<Vec<String>>, sqlx::Error> {
    let start = if ts.first().is_some_and(|t| t.keyword("constraint")) {
        2
    } else {
        0
    };
    if !ts.get(start).is_some_and(|t| t.keyword("unique")) {
        return Ok(None);
    }
    if !ts.get(start + 1).is_some_and(|t| punctuation(sql, t, "(")) {
        return Err(unsupported());
    }
    let mut columns = Vec::new();
    let mut i = start + 2;
    loop {
        columns.push(
            ts.get(i)
                .and_then(|t| t.ident.as_ref())
                .ok_or_else(unsupported)?
                .to_ascii_lowercase(),
        );
        i += 1;
        if ts.get(i).is_some_and(|t| t.keyword("collate")) {
            i += 1;
            if ts.get(i).and_then(|t| t.ident.as_ref()).is_none() {
                return Err(unsupported());
            }
            i += 1;
        }
        if ts
            .get(i)
            .is_some_and(|t| t.keyword("asc") || t.keyword("desc"))
        {
            i += 1;
        }
        let t = ts.get(i).ok_or_else(unsupported)?;
        if punctuation(sql, t, ")") {
            break;
        }
        if !punctuation(sql, t, ",") {
            return Err(unsupported());
        }
        i += 1;
    }
    // Only ordinary column-list UNIQUE constraints, optionally ON CONFLICT.
    let tail = &ts[i + 1..];
    if !(tail.is_empty()
        || (tail.len() == 3
            && tail[0].keyword("on")
            && tail[1].keyword("conflict")
            && ["rollback", "abort", "fail", "ignore", "replace"]
                .iter()
                .any(|w| tail[2].keyword(w))))
    {
        return Err(unsupported());
    }
    Ok(Some(columns))
}
fn rewrite(sql: &str, temporary: &str, before: &Keys, after: &Keys) -> Result<String, sqlx::Error> {
    let ts = tokens(sql)?;
    if ts.len() < 5 || !ts[0].keyword("create") || !ts[1].keyword("table") {
        return Err(unsupported());
    }
    let name = if ts.get(2).is_some_and(|t| t.keyword("if")) {
        if !ts.get(3).is_some_and(|t| t.keyword("not"))
            || !ts.get(4).is_some_and(|t| t.keyword("exists"))
        {
            return Err(unsupported());
        }
        5
    } else {
        2
    };
    if ts.get(name).and_then(|t| t.ident.as_ref()).is_none()
        || !ts.get(name + 1).is_some_and(|t| punctuation(sql, t, "("))
    {
        return Err(unsupported());
    }
    let open = name + 1;
    let mut depth = 1;
    let mut begin = open + 1;
    let mut byte_begin = ts[open].span.end;
    let mut parts = Vec::new();
    let mut found = BTreeSet::new();
    let mut closing = None;
    for i in open + 1..ts.len() {
        if punctuation(sql, &ts[i], "(") {
            depth += 1;
        }
        if punctuation(sql, &ts[i], ")") {
            depth -= 1;
        }
        if depth == 0 || (depth == 1 && punctuation(sql, &ts[i], ",")) {
            if begin == i {
                return Err(unsupported());
            }
            let columns = key(sql, &ts[begin..i])?;
            if let Some(ref columns) = columns {
                if columns.len() > 1 {
                    found.insert(columns.clone());
                }
            }
            let removed = columns
                .as_ref()
                .is_some_and(|k| before.contains(k) && !after.contains(k));
            if !removed {
                parts.push(sql[byte_begin..ts[i].span.start].to_string());
            }
            if depth == 0 {
                closing = Some(i);
                break;
            }
            begin = i + 1;
            byte_begin = ts[i].span.end;
        }
    }
    let close = closing.ok_or_else(unsupported)?;
    let mut tail = &ts[close + 1..];
    if tail.last().is_some_and(|t| punctuation(sql, t, ";")) {
        tail = &tail[..tail.len() - 1];
    }
    if !(tail.is_empty() || (tail.len() == 1 && tail[0].keyword("strict"))) {
        return Err(unsupported());
    }
    // A missing physical UNIQUE is allowed only when the PK subsumed it (the
    // planner filters that case). Otherwise refuse stale/ambiguous evidence.
    if before.iter().any(|k| !found.contains(k)) {
        return Err(unsupported());
    }
    for columns in after.iter().filter(|key| !before.contains(key)) {
        if columns.len() < 2 {
            return Err(unsupported());
        }
        parts.push(format!(
            " UNIQUE ({})",
            columns
                .iter()
                .map(|s| DatabaseBackend::Sqlite.quote_identifier(s))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(format!(
        "CREATE TABLE main.{} ({}){}",
        DatabaseBackend::Sqlite.quote_identifier(temporary),
        parts.join(","),
        sql[ts[close].span.start..]
            .strip_prefix(')')
            .ok_or_else(unsupported)?
    ))
}

// Split only at top-level commas. The live clause bytes remain authoritative.
fn clauses(sql: &str) -> Result<(Vec<String>, String), sqlx::Error> {
    let ts = tokens(sql)?;
    if ts.len() < 4 || !ts[0].keyword("create") || !ts[1].keyword("table") {
        return Err(unsupported());
    }
    let open = ts
        .iter()
        .position(|t| punctuation(sql, t, "("))
        .ok_or_else(unsupported)?;
    let mut depth = 1;
    let mut begin = ts[open].span.end;
    let mut parts = Vec::new();
    for t in &ts[open + 1..] {
        if punctuation(sql, t, "(") {
            depth += 1;
        }
        if punctuation(sql, t, ")") {
            depth -= 1;
        }
        if depth == 0 || (depth == 1 && punctuation(sql, t, ",")) {
            parts.push(sql[begin..t.span.start].to_string());
            begin = t.span.end;
            if depth == 0 {
                return Ok((parts, sql[t.span.end..].to_string()));
            }
        }
    }
    Err(unsupported())
}
fn signature(sql: &str) -> Result<Vec<String>, sqlx::Error> {
    Ok(tokens(sql)?
        .iter()
        .map(|t| {
            // SQLite accepts double-quoted string literals in some expression
            // contexts. Never fold their bytes as if proven identifiers.
            if t.quoted {
                return sql[t.span.clone()].to_string();
            }
            t.ident
                .as_ref()
                .map(|s| s.to_ascii_lowercase())
                .unwrap_or_else(|| sql[t.span.clone()].to_string())
        })
        .collect())
}
fn column_names(sql: &str) -> Result<BTreeSet<String>, sqlx::Error> {
    let (parts, _) = clauses(sql)?;
    let mut names = BTreeSet::new();
    for part in parts {
        let ts = tokens(&part)?;
        let first = ts.first().ok_or_else(unsupported)?;
        if !["constraint", "primary", "unique", "check", "foreign"]
            .iter()
            .any(|s| first.keyword(s))
        {
            names.insert(
                first
                    .ident
                    .as_ref()
                    .ok_or_else(unsupported)?
                    .to_ascii_lowercase(),
            );
        }
    }
    Ok(names)
}
fn rewrite_table(sql: &str, temporary: &str, old: &str, new: &str) -> Result<String, sqlx::Error> {
    // First validate the live CREATE shape/tail using the established parser.
    rewrite(sql, temporary, &vec![], &vec![])?;
    let (mut live, tail) = clauses(sql)?;
    let (old, _) = clauses(old)?;
    let (new, _) = clauses(new)?;
    let old = old
        .iter()
        .map(|s| signature(s))
        .collect::<Result<Vec<_>, _>>()?;
    let new_signatures = new
        .iter()
        .map(|s| signature(s))
        .collect::<Result<Vec<_>, _>>()?;
    let primary_members = old
        .iter()
        .filter(|s| s.iter().any(|t| t == "primary"))
        .flatten()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut replaced = BTreeSet::new();
    for removed in old.iter().filter(|s| !new_signatures.contains(s)) {
        // Changing primary keys could change SQLite's rowid alias contract.
        if primary_members.contains(&removed[0])
            || removed
                .iter()
                .any(|s| s == "primary" || s == "autoincrement")
        {
            return Err(unsupported());
        }
        let matches = live
            .iter()
            .enumerate()
            .filter_map(|(i, s)| (signature(s).ok().as_ref() == Some(removed)).then_some(i))
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(unsupported());
        }
        let column =
            !["constraint", "primary", "unique", "check", "foreign"].contains(&removed[0].as_str());
        if column {
            if let Some(replacement) = new_signatures
                .iter()
                .position(|s| s.first() == removed.first())
            {
                live[matches[0]] = new[replacement].clone();
                replaced.insert(replacement);
            } else {
                live.remove(matches[0]);
                if live
                    .iter()
                    .any(|s| signature(s).map_or(true, |ts| ts.contains(&removed[0])))
                {
                    return Err(unsupported());
                }
            }
        } else {
            live.remove(matches[0]);
        }
    }
    for (i, (part, sig)) in new.iter().zip(&new_signatures).enumerate() {
        if !old.contains(sig) && !replaced.contains(&i) {
            if sig.iter().any(|s| s == "primary" || s == "autoincrement") {
                return Err(unsupported());
            }
            let column =
                !["constraint", "primary", "unique", "check", "foreign"].contains(&sig[0].as_str());
            if column {
                live.insert(0, part.clone());
            } else {
                live.push(part.clone());
            }
        }
    }
    Ok(format!(
        "CREATE TABLE main.{} ({}){}",
        DatabaseBackend::Sqlite.quote_identifier(temporary),
        live.join(","),
        tail
    ))
}

// Incoming FKs may belong to tables excluded by managed-table-only planning.
// Removing their only referenced key cannot preserve that enforcement contract.
async fn validate_incoming_keys(
    conn: &mut sqlx::SqliteConnection,
    table: &str,
    before: &Keys,
    after: &Keys,
) -> Result<(), sqlx::Error> {
    let members = |key: &[String]| {
        key.iter()
            .map(|s| s.to_ascii_lowercase())
            .collect::<BTreeSet<_>>()
    };
    let primary: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info(?, 'main') WHERE pk>0 ORDER BY pk")
            .bind(table)
            .fetch_all(&mut *conn)
            .await?;
    let primary = primary
        .into_iter()
        .map(|s| s.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let removed = before
        .iter()
        .filter(|k| !after.contains(k))
        .map(|k| members(k))
        .collect::<Vec<_>>();
    let rows = sqlx::query(r#"SELECT m.name AS child, f.id AS id, f."to" AS target_column FROM main.sqlite_master m JOIN pragma_foreign_key_list(m.name, 'main') f WHERE m.type='table' AND f."table"=? COLLATE NOCASE ORDER BY m.name,f.id,f.seq"#).bind(table).fetch_all(&mut *conn).await?;
    let mut groups = std::collections::BTreeMap::<(String, i64), Vec<String>>::new();
    for row in rows {
        if let Some(column) = row.try_get::<Option<String>, _>("target_column")? {
            groups
                .entry((row.try_get("child")?, row.try_get("id")?))
                .or_default()
                .push(column);
        }
    }
    for columns in groups.values() {
        let columns = columns
            .iter()
            .map(|s| s.to_ascii_lowercase())
            .collect::<Vec<_>>();
        // The ORM FK contract requires an exact ordered key, even where
        // SQLite itself accepts a permuted index. Do not create a schema
        // that this workspace's introspection/validation cannot represent.
        if removed.contains(&members(&columns)) && columns != primary && !after.contains(&columns) {
            return Err(unsupported());
        }
    }
    if sqlx::query("PRAGMA main.foreign_key_check")
        .fetch_optional(&mut *conn)
        .await?
        .is_some()
    {
        return Err(unsupported());
    }
    Ok(())
}

pub(crate) struct Prepared {
    table: String,
    temporary: String,
    create: String,
    writable: Vec<String>,
    all: Vec<String>,
    rowid: String,
    schema_objects: Vec<String>,
    expected: Keys,
    sequence: Option<Option<i64>>,
}
pub(crate) async fn preflight<S: AsRef<str>>(
    pool: &sqlx::SqlitePool,
    statements: &[S],
) -> Result<(), sqlx::Error> {
    use sqlx::Connection;
    if !statements.iter().any(|s| directive(s.as_ref()).is_some()) {
        return Ok(());
    }
    let mut connection = pool.acquire().await?;
    let mut tx = connection.begin().await?;
    for statement in statements {
        if let Some(input) = directive(statement.as_ref()) {
            prepare(&mut tx, input?).await.map_err(|_| unsupported())?;
        }
    }
    tx.rollback().await?;
    Ok(())
}

// All directives are prepared before the executor runs any migration statement.
pub(crate) async fn prepare(
    conn: &mut sqlx::SqliteConnection,
    input: Directive,
) -> Result<Prepared, sqlx::Error> {
    let Directive {
        table,
        mut before,
        mut after,
        mixed,
        ddl,
    } = input;
    if mixed || unique_identifier(&table).1 {
        return Err(unsupported());
    }
    let table_info: Option<(String, i64)> =
        sqlx::query_as("SELECT type, wr FROM pragma_table_list() WHERE schema='main' AND name=?")
            .bind(&table)
            .fetch_optional(&mut *conn)
            .await?;
    if !matches!(table_info,Some((ref kind,0)) if kind=="table") {
        return Err(unsupported());
    }
    let views: i64 = sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM main.sqlite_master WHERE type='view') + (SELECT COUNT(*) FROM temp.sqlite_master WHERE type IN ('view','trigger'))")
        .fetch_one(&mut *conn)
        .await?;
    if views != 0 {
        return Err(unsupported());
    }
    let sql: String =
        sqlx::query_scalar("SELECT sql FROM main.sqlite_master WHERE type='table' AND name=?")
            .bind(&table)
            .fetch_one(&mut *conn)
            .await?;
    let temporary = format!("__graphql_orm_{table}_new");
    let collision: i64 = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM main.sqlite_master WHERE name=? COLLATE NOCASE UNION ALL SELECT COUNT(*) FROM temp.sqlite_master WHERE name=? COLLATE NOCASE OR name=? COLLATE NOCASE").bind(&temporary).bind(&temporary).bind(&table).fetch_all(&mut *conn).await?.into_iter().sum();
    if collision != 0 {
        return Err(unsupported());
    }
    let create = if let Some((old, new)) = &ddl {
        before = physical_keys(conn, &table).await?.into_iter().collect();
        after = before.clone();
        rewrite_table(&sql, &temporary, old, new)?
    } else {
        rewrite(&sql, &temporary, &before, &after)?
    };
    validate_incoming_keys(conn, &table, &before, &after).await?;
    sqlx::query(&format!("EXPLAIN {create}"))
        .fetch_all(&mut *conn)
        .await
        .map_err(|_| unsupported())?;
    let columns =
        sqlx::query("SELECT name, hidden FROM pragma_table_xinfo(?, 'main') ORDER BY cid")
            .bind(&table)
            .fetch_all(&mut *conn)
            .await?;
    let mut all = Vec::new();
    let mut writable = Vec::new();
    for column in columns {
        let name: String = column.try_get("name")?;
        // Dialect formatting accepts SQL-delimited spellings; raw catalog
        // names shaped like those spellings are ambiguous. Refuse them.
        if unique_identifier(&name).1 {
            return Err(unsupported());
        }
        match column.try_get::<i64, _>("hidden")? {
            0 => writable.push(name.clone()),
            2 | 3 => {}
            _ => return Err(unsupported()),
        }
        all.push(name);
    }
    if let Some((old, new)) = &ddl {
        let old_columns = column_names(old)?;
        let new_columns = column_names(new)?;
        let removed = old_columns.difference(&new_columns).collect::<Vec<_>>();
        if !removed.is_empty() {
            let dependent_objects: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM main.sqlite_master WHERE tbl_name=? AND type IN ('index','trigger') AND sql IS NOT NULL").bind(&table).fetch_one(&mut *conn).await?;
            let incoming: i64 = sqlx::query_scalar(r#"SELECT COUNT(*) FROM main.sqlite_master m JOIN pragma_foreign_key_list(m.name, 'main') f WHERE m.type='table' AND f."table"=? COLLATE NOCASE"#).bind(&table).fetch_one(&mut *conn).await?;
            if dependent_objects != 0 || incoming != 0 {
                return Err(unsupported());
            }
            writable.retain(|c| !removed.contains(&&c.to_ascii_lowercase()));
            all.retain(|c| !removed.contains(&&c.to_ascii_lowercase()));
        }
    }
    if after
        .iter()
        .flatten()
        .any(|key| !all.iter().any(|column| column.eq_ignore_ascii_case(key)))
    {
        return Err(unsupported());
    }
    let target_columns = ddl
        .as_ref()
        .map(|(_, new)| column_names(new))
        .transpose()?
        .unwrap_or_default();
    let rowid = ["rowid", "_rowid_", "oid"]
        .into_iter()
        .find(|name| {
            !all.iter().any(|c| c.eq_ignore_ascii_case(name)) && !target_columns.contains(*name)
        })
        .ok_or_else(unsupported)?
        .to_string();
    let schema_objects = sqlx::query_scalar("SELECT sql FROM main.sqlite_master WHERE tbl_name=? AND type IN ('index','trigger') AND sql IS NOT NULL ORDER BY type,name").bind(&table).fetch_all(&mut *conn).await?;
    let sequence = if tokens(&sql)?.iter().any(|t| t.keyword("autoincrement")) {
        Some(
            sqlx::query_scalar("SELECT seq FROM main.sqlite_sequence WHERE name=?")
                .bind(&table)
                .fetch_optional(&mut *conn)
                .await?,
        )
    } else {
        None
    };
    if physical_keys(conn, &table).await? != before.iter().cloned().collect() {
        return Err(unsupported());
    }
    Ok(Prepared {
        table,
        temporary,
        create,
        writable,
        all,
        rowid,
        schema_objects,
        expected: after,
        sequence,
    })
}
async fn physical_keys(
    conn: &mut sqlx::SqliteConnection,
    table: &str,
) -> Result<BTreeSet<Vec<String>>, sqlx::Error> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_master WHERE type='table' AND name=?)",
    )
    .bind(table)
    .fetch_one(&mut *conn)
    .await?;
    if !exists {
        return Err(unsupported());
    }
    let names: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_index_list(?, 'main') WHERE origin='u'")
            .bind(table)
            .fetch_all(&mut *conn)
            .await?;
    let mut keys = BTreeSet::new();
    for name in names {
        let columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_index_info(?, 'main') ORDER BY seqno")
                .bind(name)
                .fetch_all(&mut *conn)
                .await?;
        if columns.len() > 1 {
            keys.insert(
                columns
                    .into_iter()
                    .map(|s| s.to_ascii_lowercase())
                    .collect(),
            );
        }
    }
    Ok(keys)
}
impl Prepared {
    pub(crate) async fn apply(self, conn: &mut sqlx::SqliteConnection) -> Result<(), sqlx::Error> {
        let quote = |s: &str| DatabaseBackend::Sqlite.quote_identifier(s);
        let table = format!("main.{}", quote(&self.table));
        let temporary = format!("main.{}", quote(&self.temporary));
        let columns = std::iter::once(quote(&self.rowid))
            .chain(self.writable.iter().map(|s| quote(s)))
            .collect::<Vec<_>>()
            .join(", ");
        sqlx::query(&self.create).execute(&mut *conn).await?;
        sqlx::query(&format!(
            "INSERT INTO {temporary} ({columns}) SELECT {columns} FROM {table}"
        ))
        .execute(&mut *conn)
        .await?;
        // Generated values and SQLite storage classes must also remain exact;
        // binary comparison avoids column collations masking a changed value.
        let values = std::iter::once(quote(&self.rowid))
            .chain(self.all.iter().flat_map(|c| {
                [
                    format!("typeof({})", quote(c)),
                    format!("{} COLLATE BINARY", quote(c)),
                    format!("CAST({} AS BLOB)", quote(c)),
                ]
            }))
            .collect::<Vec<_>>()
            .join(", ");
        for (left, right) in [(&table, &temporary), (&temporary, &table)] {
            let mismatch: bool = sqlx::query_scalar(&format!(
                "SELECT EXISTS(SELECT {values} FROM {left} EXCEPT SELECT {values} FROM {right})"
            ))
            .fetch_one(&mut *conn)
            .await?;
            if mismatch {
                return Err(unsupported());
            }
        }
        sqlx::query(&format!("DROP TABLE {table}"))
            .execute(&mut *conn)
            .await?;
        sqlx::query(&format!(
            "ALTER TABLE {temporary} RENAME TO {}",
            quote(&self.table)
        ))
        .execute(&mut *conn)
        .await?;
        if let Some(sequence) = self.sequence {
            sqlx::query("DELETE FROM main.sqlite_sequence WHERE name=?")
                .bind(&self.table)
                .execute(&mut *conn)
                .await?;
            if let Some(sequence) = sequence {
                sqlx::query("INSERT INTO main.sqlite_sequence(name,seq) VALUES (?,?)")
                    .bind(&self.table)
                    .bind(sequence)
                    .execute(&mut *conn)
                    .await?;
            }
        }
        for sql in self.schema_objects {
            sqlx::query(&sql).execute(&mut *conn).await?;
        }
        if physical_keys(conn, &self.table).await? != self.expected.into_iter().collect() {
            return Err(unsupported());
        }
        Ok(())
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;

    #[tokio::test]
    async fn cancelled_live_column_rebuild_rolls_back_and_retries() -> Result<(), sqlx::Error> {
        for new in [
            "CREATE TABLE probe(id INTEGER PRIMARY KEY AUTOINCREMENT,a TEXT,g TEXT GENERATED ALWAYS AS (a || '!') STORED)",
            "CREATE TABLE probe(id INTEGER PRIMARY KEY AUTOINCREMENT,a TEXT NOT NULL,g TEXT GENERATED ALWAYS AS (a || '!') STORED, CHECK(a IS NOT NULL))",
            "CREATE TABLE probe(id INTEGER PRIMARY KEY AUTOINCREMENT,a TEXT NOT NULL,g TEXT GENERATED ALWAYS AS (a || '!') STORED, FOREIGN KEY(a) REFERENCES parent(id))",
        ] {
            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await?;
            sqlx::query("CREATE TABLE parent(id TEXT PRIMARY KEY)")
                .execute(&pool)
                .await?;
            sqlx::query("INSERT INTO parent VALUES ('kept')")
                .execute(&pool)
                .await?;
            let old = "CREATE TABLE probe(id INTEGER PRIMARY KEY AUTOINCREMENT,a TEXT NOT NULL,g TEXT GENERATED ALWAYS AS (a || '!') STORED)";
            for sql in [
                old,
                "INSERT INTO probe(id,a) VALUES (99,'kept'),(999,'deleted')",
                "DELETE FROM probe WHERE id=999",
            ] {
                sqlx::query(sql).execute(&pool).await?;
            }
            let input = move || Directive {
                table: "probe".into(),
                before: vec![],
                after: vec![],
                mixed: false,
                ddl: Some((old.into(), new.into())),
            };
            let (ready, applied) = tokio::sync::oneshot::channel();
            let task_pool = pool.clone();
            let task = tokio::spawn(async move {
                let mut tx = task_pool.begin().await.unwrap();
                prepare(&mut tx, input())
                    .await
                    .unwrap()
                    .apply(&mut tx)
                    .await
                    .unwrap();
                ready.send(()).unwrap();
                std::future::pending::<()>().await;
            });
            applied.await.unwrap();
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            let sql: String =
                sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE name='probe'")
                    .fetch_one(&pool)
                    .await?;
            assert_eq!(sql, old);
            let row: (i64, String, String) = sqlx::query_as("SELECT rowid,a,g FROM probe")
                .fetch_one(&pool)
                .await?;
            assert_eq!(row, (99, "kept".into(), "kept!".into()));
            let seq: i64 = sqlx::query_scalar("SELECT seq FROM sqlite_sequence WHERE name='probe'")
                .fetch_one(&pool)
                .await?;
            assert_eq!(seq, 999);
            let leftovers: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='__graphql_orm_probe_new'",
            )
            .fetch_one(&pool)
            .await?;
            assert_eq!(leftovers, 0);
            let mut tx = pool.begin().await?;
            prepare(&mut tx, input()).await?.apply(&mut tx).await?;
            tx.commit().await?;
        }
        Ok(())
    }
}
