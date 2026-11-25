//! PostgreSQL canonical change-journal keys and their cutover boundary.

use super::rls;
#[cfg(feature = "postgres")]
use sqlx::PgConnection;

const EPOCH_PREFIX: &str = "graphql_orm_canonical_keys_v1_";
pub(super) const ADMIN_LOCK: &str =
    "SELECT pg_catalog.pg_advisory_xact_lock(421736070207429592::pg_catalog.int8)";
#[cfg(feature = "postgres")]
const JOURNAL: &str = "\"__graphql_orm_change_log\"";

// Only built-in scalar types with a known binary format are allowed. In
// particular, record_send must never call a user-defined type's send function
// from the SECURITY DEFINER recorder.
const KEY_TYPES: &str = "ARRAY['pg_catalog.bool'::pg_catalog.regtype, 'pg_catalog.int2'::pg_catalog.regtype, 'pg_catalog.int4'::pg_catalog.regtype, 'pg_catalog.int8'::pg_catalog.regtype, 'pg_catalog.float4'::pg_catalog.regtype, 'pg_catalog.float8'::pg_catalog.regtype, 'pg_catalog.numeric'::pg_catalog.regtype, 'pg_catalog.uuid'::pg_catalog.regtype, 'pg_catalog.timestamp'::pg_catalog.regtype, 'pg_catalog.timestamptz'::pg_catalog.regtype, 'pg_catalog.date'::pg_catalog.regtype, 'pg_catalog.text'::pg_catalog.regtype, 'pg_catalog.varchar'::pg_catalog.regtype, 'pg_catalog.bytea'::pg_catalog.regtype, 'pg_catalog.jsonb'::pg_catalog.regtype]";
const UNSUPPORTED_KEY: &str =
    "canonical change journal keys require a supported PostgreSQL scalar primary-key type";

/// The identical SQL projection for a live snapshot column and OLD/NEW in
/// the trigger. Text is explicitly UTF-8 rather than in the client encoding
/// used by textsend. JSONB's binary protocol also contains encoded text, so
/// use explicit UTF-8 for it. Numeric scale is immaterial to numeric equality.
pub(super) fn projection(key: &str) -> String {
    // The single-field record header is three int32s (count, type OID,
    // length). Strip it for bytea to preserve the binary journal spelling.
    // Guard the input to the numeric cast itself. PostgreSQL can fold casts
    // of a trigger record's constant fields while planning an unused branch.
    format!(
        "CASE pg_catalog.pg_typeof({key})
         WHEN 'pg_catalog.bytea'::pg_catalog.regtype THEN 'bin:' OPERATOR(pg_catalog.||) pg_catalog.encode(pg_catalog.substr(pg_catalog.record_send(ROW({key})), 13), 'hex')
         WHEN 'pg_catalog.text'::pg_catalog.regtype THEN 'pg1:' OPERATOR(pg_catalog.||) pg_catalog.encode(pg_catalog.convert_to({key}::pg_catalog.text, 'UTF8'), 'hex')
         WHEN 'pg_catalog.varchar'::pg_catalog.regtype THEN 'pg1:' OPERATOR(pg_catalog.||) pg_catalog.encode(pg_catalog.convert_to({key}::pg_catalog.text, 'UTF8'), 'hex')
         WHEN 'pg_catalog.jsonb'::pg_catalog.regtype THEN 'pg1:' OPERATOR(pg_catalog.||) pg_catalog.encode(pg_catalog.convert_to({key}::pg_catalog.text, 'UTF8'), 'hex')
         WHEN 'pg_catalog.numeric'::pg_catalog.regtype THEN 'pg1:' OPERATOR(pg_catalog.||) pg_catalog.encode(pg_catalog.numeric_send(pg_catalog.trim_scale((CASE WHEN pg_catalog.pg_typeof({key}) OPERATOR(pg_catalog.=) 'pg_catalog.numeric'::pg_catalog.regtype THEN {key}::pg_catalog.text END)::pg_catalog.numeric)), 'hex')
         ELSE 'pg1:' OPERATOR(pg_catalog.||) pg_catalog.encode(pg_catalog.record_send(ROW({key})), 'hex') END"
    )
}

/// Keep the legacy recorder definition byte-for-byte. The upgraded variant
/// retains every ownership and trigger check and only changes rows belonging
/// to a journal that has an epoch.
pub(super) fn recorder_body() -> String {
    let projection = rls::sql_string_literal(&projection("($1).%1$I"));
    rls::CHANGE_JOURNAL_FUNCTION_BODY
        .replacen(
            "DECLARE entry_key",
            "DECLARE canonical pg_catalog.bool; entry_key",
            1,
        )
        .replacen(
            "IF TG_ARGV[6] = 'bin' THEN",
            &format!(
                "canonical := EXISTS (SELECT 1 FROM pg_catalog.pg_constraint c WHERE c.conrelid = journal AND {epoch_predicate}); \
                 IF canonical THEN \
                   IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_attribute a WHERE a.attrelid = TG_RELID AND a.attname::pg_catalog.text = TG_ARGV[5] AND a.atttypid = ANY ({KEY_TYPES})) THEN RAISE EXCEPTION '{UNSUPPORTED_KEY}'; END IF; \
                   key_sql := 'SELECT ' || {projection}; \
                 ELSIF TG_ARGV[6] = 'bin' THEN",
                epoch_predicate = epoch_predicate(),
            ),
            1,
        )
        .replacen(
            "IF TG_OP = 'UPDATE' AND previous_key IS NOT DISTINCT FROM entry_key THEN",
            "IF canonical THEN entry_sql := pg_catalog.replace(pg_catalog.replace(entry_sql, 'action, changed_at)', 'action, changed_at, canonical_key)'), '$5)', '$5, true)'); END IF; \
             IF TG_OP = 'UPDATE' AND (CASE WHEN canonical THEN previous_key COLLATE pg_catalog.\"C\" IS NOT DISTINCT FROM entry_key COLLATE pg_catalog.\"C\" ELSE previous_key IS NOT DISTINCT FROM entry_key END) THEN",
            1,
        )
}

fn recorder_definition(body: &str) -> String {
    format!(
        "CREATE OR REPLACE FUNCTION graphql_orm.{}() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, pg_temp AS $graphql_orm_body${body}$graphql_orm_body$",
        rls::CHANGE_JOURNAL_FUNCTION,
    )
}

// The cutover constraint persists the boundary in the journal's own
// catalog metadata. It travels with the journal OID, requires no extra
// privileges, and cannot be impersonated through the session search path.
fn epoch_predicate() -> String {
    format!(
        "c.contype OPERATOR(pg_catalog.=) 'c' AND pg_catalog.starts_with(c.conname::pg_catalog.text, '{EPOCH_PREFIX}')"
    )
}

pub(super) fn recorder_install_sql() -> String {
    let legacy = recorder_definition(rls::CHANGE_JOURNAL_FUNCTION_BODY);
    let canonical = recorder_definition(&recorder_body());
    let epoch = epoch_predicate();
    format!(
        "DO $graphql_orm$ BEGIN \
         IF EXISTS (SELECT 1 FROM pg_catalog.pg_constraint c JOIN pg_catalog.pg_class j ON j.oid OPERATOR(pg_catalog.=) c.conrelid \
                    WHERE {epoch} AND j.relname OPERATOR(pg_catalog.=) '__graphql_orm_change_log' \
                    AND pg_catalog.pg_get_userbyid(j.relowner) OPERATOR(pg_catalog.=) CURRENT_USER) THEN {canonical}; \
         ELSE {legacy}; END IF; END $graphql_orm$"
    )
}

/// Check the live key type when installing a trigger. The trigger DDL has
/// already locked the target, so the column cannot change before commit.
pub(super) fn install_type_check(key_literal: &str) -> String {
    format!(
        "IF EXISTS (SELECT 1 FROM pg_catalog.pg_constraint c WHERE c.conrelid OPERATOR(pg_catalog.=) journal AND {epoch}) \
         AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_attribute a WHERE a.attrelid OPERATOR(pg_catalog.=) target \
           AND a.attname::pg_catalog.text OPERATOR(pg_catalog.=) {key_literal} \
           AND a.atttypid OPERATOR(pg_catalog.=) ANY ({KEY_TYPES}) AND NOT a.attisdropped) \
         THEN RAISE EXCEPTION '{UNSUPPORTED_KEY}: %.%', target, {key_literal}; END IF;",
        epoch = epoch_predicate(),
    )
}

/// Read the type OID (which includes the width of fixed-width types) and the
/// type modifier (e.g. numeric precision/scale) of a supported live key.
#[cfg(feature = "postgres")]
pub(super) async fn key_type(
    conn: &mut PgConnection,
    table: &str,
    column: &str,
) -> crate::Result<(i64, i32)> {
    let (type_oid, modifier, supported): (i64, i32, bool) = sqlx::query_as(&format!(
        "SELECT a.atttypid::pg_catalog.int8, a.atttypmod,
                a.atttypid OPERATOR(pg_catalog.=) ANY ({KEY_TYPES})
         FROM pg_catalog.pg_attribute a
         WHERE a.attrelid OPERATOR(pg_catalog.=) pg_catalog.to_regclass($1)::pg_catalog.oid
           AND a.attname::pg_catalog.text OPERATOR(pg_catalog.=) $2
           AND NOT a.attisdropped"
    ))
    .bind(table)
    .bind(column)
    .fetch_optional(conn)
    .await?
    .ok_or_else(|| sqlx::Error::Protocol(format!(
        "canonical key table or column not found: {table}.{column}; check that the database schema is migrated"
    )))?;
    if supported {
        Ok((type_oid, modifier))
    } else {
        Err(sqlx::Error::Protocol(format!(
            "{UNSUPPORTED_KEY}: {table}.{column}"
        )))
    }
}

#[cfg(feature = "postgres")]
async fn registered_tables(conn: &mut PgConnection) -> crate::Result<Vec<(String, String)>> {
    sqlx::query_as(&format!(
        "SELECT pg_catalog.format('%I.%I', n.nspname, c.relname), r.key_column
         FROM {registry} r
         JOIN pg_catalog.pg_class c ON c.oid OPERATOR(pg_catalog.=) r.relid::pg_catalog.oid
         JOIN pg_catalog.pg_namespace n ON n.oid OPERATOR(pg_catalog.=) c.relnamespace
         WHERE r.journal_schema OPERATOR(pg_catalog.=) (
            SELECT jn.nspname::pg_catalog.text FROM pg_catalog.pg_class j
            JOIN pg_catalog.pg_namespace jn ON jn.oid OPERATOR(pg_catalog.=) j.relnamespace
            WHERE j.oid OPERATOR(pg_catalog.=) pg_catalog.to_regclass($1)::pg_catalog.oid)
         ORDER BY c.oid",
        registry = rls::CHANGE_JOURNAL_REGISTRY,
    ))
    .bind(JOURNAL)
    .fetch_all(conn)
    .await
}

#[cfg(feature = "postgres")]
pub(super) async fn epoch(conn: &mut PgConnection) -> crate::Result<Option<i64>> {
    let (epochs, has_marker_column): (Vec<String>, bool) = sqlx::query_as(&format!(
        "SELECT ARRAY(
             SELECT pg_catalog.substr(c.conname::pg_catalog.text, {number_start})
             FROM pg_catalog.pg_constraint c
             WHERE c.conrelid OPERATOR(pg_catalog.=) journal.oid AND {epoch}
         ), EXISTS (
             SELECT 1 FROM pg_catalog.pg_attribute a
             WHERE a.attrelid OPERATOR(pg_catalog.=) journal.oid AND NOT a.attisdropped
               AND a.attname OPERATOR(pg_catalog.=) 'canonical_key'
         )
         FROM (SELECT pg_catalog.to_regclass($1)::pg_catalog.oid AS oid) journal",
        number_start = EPOCH_PREFIX.len() + 1,
        epoch = epoch_predicate(),
    ))
    .bind(JOURNAL)
    .fetch_one(&mut *conn)
    .await?;
    let start = match (epochs.as_slice(), has_marker_column) {
        ([], false) => None,
        ([start], true) => start
            .parse::<i64>()
            .ok()
            .filter(|start| *start > 0)
            .map(Some)
            .ok_or_else(invalid_epoch)?,
        _ => return Err(invalid_epoch()),
    };
    if start.is_some() {
        let installed: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_proc
             WHERE oid OPERATOR(pg_catalog.=) pg_catalog.to_regprocedure($1)::pg_catalog.oid
             AND prosrc COLLATE pg_catalog.\"C\" OPERATOR(pg_catalog.=) $2 AND prosecdef
             AND proconfig OPERATOR(pg_catalog.=) ARRAY[$3]::pg_catalog.text[])",
        )
        .bind(format!("graphql_orm.{}()", rls::CHANGE_JOURNAL_FUNCTION))
        .bind(recorder_body())
        .bind(rls::CHANGE_JOURNAL_FUNCTION_CONFIG)
        .fetch_one(&mut *conn)
        .await?;
        if !installed {
            return Err(sqlx::Error::Protocol(
                "canonical change journal recorder is missing or altered; re-apply the schema target".into(),
            ));
        }
    }
    Ok(start)
}

#[cfg(feature = "postgres")]
pub(super) async fn enable(pool: &sqlx::PgPool) -> crate::Result<i64> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await?;
    sqlx::query(ADMIN_LOCK).execute(&mut *tx).await?;
    if let Some(start) = epoch(&mut tx).await? {
        tx.commit().await?;
        return Ok(start);
    }
    let managed: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM pg_catalog.pg_class j
            JOIN pg_catalog.pg_proc p ON p.oid OPERATOR(pg_catalog.=) pg_catalog.to_regprocedure($2)::pg_catalog.oid
            WHERE j.oid OPERATOR(pg_catalog.=) pg_catalog.to_regclass($1)::pg_catalog.oid
              AND j.relkind OPERATOR(pg_catalog.=) 'r' AND j.relpersistence OPERATOR(pg_catalog.<>) 't'
              AND pg_catalog.pg_get_userbyid(j.relowner) OPERATOR(pg_catalog.=) CURRENT_USER
              AND j.relowner OPERATOR(pg_catalog.=) p.proowner AND p.prosecdef
              AND p.prosrc COLLATE pg_catalog.\"C\" OPERATOR(pg_catalog.=) ANY (ARRAY[$3, $4]::pg_catalog.text[])
              AND p.proconfig OPERATOR(pg_catalog.=) ARRAY[$5]::pg_catalog.text[]
        )",
    )
    .bind(JOURNAL)
    .bind(format!("graphql_orm.{}()", rls::CHANGE_JOURNAL_FUNCTION))
    .bind(rls::CHANGE_JOURNAL_FUNCTION_BODY)
    .bind(recorder_body())
    .bind(rls::CHANGE_JOURNAL_FUNCTION_CONFIG)
    .fetch_one(&mut *tx)
    .await?;
    if !managed {
        return Err(sqlx::Error::Protocol(
            "canonical change journal cutover requires the existing ORM recorder; install it and run the cutover as the journal owner".into(),
        ));
    }
    // Take the entity-table locks before the journal lock, in writer order.
    // Stop all writers and backup workers for the operational cutover, which
    // also needs to establish a fresh full snapshot and matching checkpoint.
    for (path, column) in registered_tables(&mut tx).await? {
        sqlx::query(&format!("LOCK TABLE {path} IN SHARE MODE"))
            .execute(&mut *tx)
            .await?;
        key_type(&mut tx, &path, &column).await?;
    }
    sqlx::query(&format!("LOCK TABLE {JOURNAL} IN ACCESS EXCLUSIVE MODE"))
        .execute(&mut *tx)
        .await?;
    let start: i64 = sqlx::query_scalar(
        "SELECT pg_catalog.nextval(s.seqrelid::pg_catalog.regclass) FROM pg_catalog.pg_sequence s
         WHERE s.seqrelid OPERATOR(pg_catalog.=) pg_catalog.pg_get_serial_sequence($1, 'seq')::pg_catalog.regclass
           AND s.seqcache OPERATOR(pg_catalog.=) 1 AND s.seqincrement OPERATOR(pg_catalog.=) 1 AND NOT s.seqcycle",
    )
    .bind(JOURNAL)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| sqlx::Error::Protocol(
        "canonical change journal cutover requires the ORM's non-cycling identity sequence with increment and cache size one".into(),
    ))?;
    let legacy_max: Option<i64> =
        sqlx::query_scalar(&format!("SELECT pg_catalog.max(seq) FROM {JOURNAL}"))
            .fetch_one(&mut *tx)
            .await?;
    if start <= 0 || legacy_max.is_some_and(|position| position >= start) {
        return Err(sqlx::Error::Protocol(
            "canonical change journal cutover could not reserve a position above all legacy entries; repair the journal sequence before retrying".into(),
        ));
    }
    // Reserve a position above every legacy cursor, including cursors into
    // pruned history. The constraint also refuses writes by a legacy recorder
    // if an older deployment accidentally replaces the canonical function.
    sqlx::query(&format!(
        "ALTER TABLE {JOURNAL} ADD COLUMN canonical_key pg_catalog.bool NOT NULL DEFAULT false,
         ADD CONSTRAINT {EPOCH_PREFIX}{start} CHECK (seq OPERATOR(pg_catalog.<) {start} OR canonical_key) NOT VALID"
    ))
    .execute(&mut *tx)
    .await?;
    sqlx::query(&recorder_definition(&recorder_body()))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(start)
}

#[cfg(feature = "postgres")]
fn invalid_epoch() -> sqlx::Error {
    sqlx::Error::Protocol("canonical change journal boundary is invalid".into())
}
