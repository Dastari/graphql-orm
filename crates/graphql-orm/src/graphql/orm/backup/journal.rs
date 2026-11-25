use super::{CHANGE_JOURNAL_TABLE, quote_identifier};
use crate::graphql::orm;
use orm::{EntityMetadata, MutationEvent, MutationPhase, SqlValue, current_backend};

pub(crate) async fn record_change_journal_event<B>(
    hook_ctx: &mut orm::MutationContext<'_, B>,
    event: &MutationEvent,
) -> crate::Result<()>
where
    B: orm::WriteBackend,
{
    if !cfg!(feature = "change-journal")
        || !hook_ctx.policy_database().change_journal_enabled()
        || event.phase != MutationPhase::After
    {
        return Ok(());
    }
    // The journal identifies rows by entity id, which cannot name a composite key; recording
    // such a row would make incremental chains resurrect or duplicate it. Fail closed.
    if event.metadata.primary_keys.len() > 1 {
        return Err(sqlx::Error::Protocol(format!(
            "the change journal cannot record composite-key entity {}; back it up with full snapshots",
            event.entity_name
        )));
    }
    if B::DIALECT == orm::DatabaseBackend::Postgres {
        return verify_postgres_change_journal_recorder(hook_ctx, event).await;
    }

    let action = match event.action {
        orm::ChangeAction::Created => "create",
        orm::ChangeAction::Updated => "update",
        orm::ChangeAction::Deleted => "delete",
    };
    let changed_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    let sql = format!(
        "INSERT INTO {} (
            id,
            entity_name,
            table_name,
            primary_key,
            action,
            changed_at,
            transaction_id,
            row_hash,
            actor_id,
            correlation_id
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        quote_identifier(CHANGE_JOURNAL_TABLE)
    );
    let values = vec![
        SqlValue::Uuid(uuid::Uuid::new_v4()),
        SqlValue::String(event.entity_name.to_string()),
        SqlValue::String(event.table_name.to_string()),
        SqlValue::String(event.id.clone()),
        SqlValue::String(action.to_string()),
        SqlValue::Int(changed_at),
        SqlValue::StringNull,
        SqlValue::StringNull,
        SqlValue::StringNull,
        SqlValue::StringNull,
    ];
    hook_ctx.execute(&sql, &values).await?;
    Ok(())
}

/// Turn on database-side journal recording for an ORM write transaction.
///
/// On PostgreSQL the recorder trigger records changes only in transactions
/// that set [`orm::rls::CHANGE_JOURNAL_SETTING`]. The setting selects
/// transactions; it cannot name the entity, key or action of an entry. Restore
/// imports never set it, so they do not journal.
pub(crate) async fn begin_change_journal_transaction<B>(
    db: &crate::db::Database<B>,
    tx: &mut B::Transaction<'_>,
) -> crate::Result<()>
where
    B: orm::WriteBackend,
{
    if !cfg!(feature = "change-journal")
        || B::DIALECT != orm::DatabaseBackend::Postgres
        || current_backend() != orm::DatabaseBackend::Postgres
        || !db.change_journal_enabled()
    {
        return Ok(());
    }
    // A utility statement: unlike `SELECT set_config(...)` it takes no MVCC
    // snapshot, so callers that must lock before their first snapshot (such
    // as dependent deletes) are unaffected.
    B::execute_in_transaction(
        tx,
        &format!("SET LOCAL {} = 'on'", orm::rls::CHANGE_JOURNAL_SETTING),
        &[],
    )
    .await?;
    Ok(())
}

impl<B> orm::MutationContext<'_, B>
where
    B: orm::WriteBackend,
{
    /// Check that PostgreSQL's change journal recorder will record writes to
    /// `metadata`'s table in this transaction.
    ///
    /// Journaled ORM writes run this check once per table before they rely on
    /// the recorder. Hosts can call it to fail fast at startup. It returns the
    /// specific reason on failure. On SQLite it always succeeds.
    ///
    /// A current role that acts as the journal owner (is a member of the
    /// owning role, as a superuser always is) passes the privilege part of the
    /// check, because it can already do anything the checked privileges would
    /// allow. `validate_target` reports such a role; run it with the runtime
    /// pool.
    #[doc(hidden)]
    pub async fn check_change_journal_recorder(
        &mut self,
        metadata: &'static EntityMetadata,
    ) -> crate::Result<()> {
        if B::DIALECT != orm::DatabaseBackend::Postgres {
            return Ok(());
        }
        check_postgres_change_journal_recorder(self, metadata).await
    }
}

/// Fail closed unless the PostgreSQL recorder records `event`'s table in this
/// transaction. The check runs once per table per transaction.
async fn verify_postgres_change_journal_recorder<B>(
    hook_ctx: &mut orm::MutationContext<'_, B>,
    event: &MutationEvent,
) -> crate::Result<()>
where
    B: orm::WriteBackend,
{
    if hook_ctx
        .change_journal_verified_tables
        .contains(&event.table_name)
    {
        return Ok(());
    }
    check_postgres_change_journal_recorder(hook_ctx, event.metadata).await?;
    hook_ctx
        .change_journal_verified_tables
        .push(event.table_name);
    Ok(())
}

/// Privilege condition shared by the recorder check and its diagnosis: the
/// current role either acts as the journal owner (is a member of it), or
/// holds no table or column privilege on the journal beyond `SELECT` and no
/// `EXECUTE` on the recorder, directly, through `PUBLIC`, or through any role
/// it is a member of, including `NOINHERIT` memberships it can `SET ROLE` to.
/// A role with `TRIGGER` on the journal could run code as its owner, one with
/// `INSERT` or `UPDATE` could write entries if it bypasses RLS, and one with
/// `EXECUTE` on the recorder could attach it to other triggers.
const RECORDER_PRIVILEGES_CLEAN: &str = "(pg_catalog.pg_has_role(CURRENT_USER, j.relowner, 'MEMBER')
          OR (NOT EXISTS (
                  SELECT 1
                  FROM (SELECT j.relacl AS acl
                        UNION ALL
                        SELECT att.attacl FROM pg_catalog.pg_attribute att
                        WHERE att.attrelid OPERATOR(pg_catalog.=) j.oid AND att.attnum OPERATOR(pg_catalog.>) 0 AND NOT att.attisdropped) acls
                  CROSS JOIN LATERAL pg_catalog.aclexplode(acls.acl) acl
                  WHERE acl.privilege_type OPERATOR(pg_catalog.<>) 'SELECT' AND acl.grantee OPERATOR(pg_catalog.<>) j.relowner
                    AND CASE WHEN acl.grantee OPERATOR(pg_catalog.=) 0::pg_catalog.oid THEN true
                             ELSE pg_catalog.pg_has_role(CURRENT_USER, acl.grantee, 'MEMBER') END)
              AND NOT EXISTS (
                  SELECT 1
                  FROM pg_catalog.aclexplode(COALESCE(p.proacl, pg_catalog.acldefault('f', p.proowner))) acl
                  WHERE acl.privilege_type OPERATOR(pg_catalog.=) 'EXECUTE' AND acl.grantee OPERATOR(pg_catalog.<>) p.proowner
                    AND CASE WHEN acl.grantee OPERATOR(pg_catalog.=) 0::pg_catalog.oid THEN true
                             ELSE pg_catalog.pg_has_role(CURRENT_USER, acl.grantee, 'MEMBER') END)))";

/// The registry is owned by the recorder's owner and grants no role other
/// than its owner anything beyond `SELECT`, at table or column level.
const REGISTRY_INTACT: &str = "EXISTS (
              SELECT 1 FROM pg_catalog.pg_class g
              WHERE g.oid OPERATOR(pg_catalog.=) pg_catalog.to_regclass('graphql_orm.change_journal_tables')
                AND g.relowner OPERATOR(pg_catalog.=) p.proowner
                AND NOT EXISTS (
                    SELECT 1
                    FROM (SELECT g.relacl AS acl
                          UNION ALL
                          SELECT att.attacl FROM pg_catalog.pg_attribute att
                          WHERE att.attrelid OPERATOR(pg_catalog.=) g.oid AND att.attnum OPERATOR(pg_catalog.>) 0 AND NOT att.attisdropped) acls
                    CROSS JOIN LATERAL pg_catalog.aclexplode(acls.acl) acl
                    WHERE acl.grantee OPERATOR(pg_catalog.<>) g.relowner AND acl.privilege_type OPERATOR(pg_catalog.<>) 'SELECT'))";

/// The recorder check for one table.
///
/// It requires the recording setting, a `session_replication_role` other
/// than `replica`, and an intact recorder trigger on the table: enabled,
/// unconditional, of the exact type, the only trigger on the table calling the
/// unmodified recorder, which the table owner owns, with arguments naming
/// this table's schema and name, entity, declared table and key, and an
/// existing journal owned by that owner that carries no trigger or rule. The
/// current role must not hold privileges that would let it run code as the
/// journal owner (see [`RECORDER_PRIVILEGES_CLEAN`]).
async fn check_postgres_change_journal_recorder<B>(
    hook_ctx: &mut orm::MutationContext<'_, B>,
    metadata: &'static EntityMetadata,
) -> crate::Result<()>
where
    B: orm::WriteBackend,
{
    // The main check names the registry directly. Probe the schema and the
    // registry first, each in its own statement, so a deployment that has
    // not applied the plan yet, or a role without `USAGE` on the schema,
    // gets a guided error rather than a raw lookup failure.
    if let Some(message) = probe_postgres_change_journal_registry(hook_ctx).await? {
        return Err(sqlx::Error::Protocol(format!(
            "{message}; writes to {} would not be journaled",
            metadata.table_name
        )));
    }
    let key = metadata
        .primary_keys
        .first()
        .copied()
        .unwrap_or(metadata.primary_key);
    let sql_type = orm::SchemaModel::from_entities(&[metadata])
        .tables
        .into_iter()
        .next()
        .and_then(|table| {
            table
                .columns
                .into_iter()
                .find(|column| column.name == key)
                .map(|column| column.sql_type)
        })
        .unwrap_or_default();
    let mut expected_arguments = Vec::new();
    for argument in [
        metadata.entity_name,
        metadata.table_name,
        key,
        orm::rls::change_journal_key_encoding(&sql_type),
    ] {
        expected_arguments.extend_from_slice(argument.as_bytes());
        expected_arguments.push(0);
    }
    // The table the ORM's own writes reach: the declared path rendered as the
    // ORM renders it. A bare name must resolve in the journal's schema.
    let table = orm::rls::change_journal_orm_path(metadata.table_name);
    let qualified =
        !orm::dialect::identifier_path_is_bare(orm::DatabaseBackend::Postgres, metadata.table_name);
    let sql = format!(
        "SELECT 1
        FROM pg_catalog.pg_trigger tr
        JOIN pg_catalog.pg_class t ON t.oid OPERATOR(pg_catalog.=) tr.tgrelid
        JOIN pg_catalog.pg_namespace tn ON tn.oid OPERATOR(pg_catalog.=) t.relnamespace
        JOIN pg_catalog.pg_proc p ON p.oid OPERATOR(pg_catalog.=) tr.tgfoid
        JOIN pg_catalog.pg_namespace pn ON pn.oid OPERATOR(pg_catalog.=) p.pronamespace
        CROSS JOIN LATERAL (
            SELECT pg_catalog.convert_from(
                pg_catalog.substr(tr.tgargs, 1, GREATEST(pg_catalog.position(tr.tgargs, '\\x00'::pg_catalog.bytea) OPERATOR(pg_catalog.-) 1, 0)),
                'UTF8'
            ) AS journal_schema,
            pg_catalog.substr(tr.tgargs, pg_catalog.position(tr.tgargs, '\\x00'::pg_catalog.bytea) OPERATOR(pg_catalog.+) 1) AS entry_arguments
        ) args
        JOIN pg_catalog.pg_class j
          ON j.oid OPERATOR(pg_catalog.=) pg_catalog.to_regclass(pg_catalog.format('%I.%I', args.journal_schema, ?))
        WHERE tr.tgrelid OPERATOR(pg_catalog.=) pg_catalog.to_regclass(?)
          AND tr.tgname OPERATOR(pg_catalog.=) ? AND NOT tr.tgisinternal AND tr.tgenabled OPERATOR(pg_catalog.=) 'O'
          AND tr.tgtype OPERATOR(pg_catalog.=) ? AND tr.tgqual IS NULL AND tr.tgnargs OPERATOR(pg_catalog.=) 7
          AND pn.nspname OPERATOR(pg_catalog.=) 'graphql_orm' AND p.proname OPERATOR(pg_catalog.=) ? AND p.pronargs OPERATOR(pg_catalog.=) 0
          AND p.prosecdef AND p.prosrc COLLATE pg_catalog.\"C\" OPERATOR(pg_catalog.=) ANY (ARRAY[?, ?]::pg_catalog.text[]) AND p.proconfig OPERATOR(pg_catalog.=) ARRAY[?]::pg_catalog.text[]
          AND p.proowner OPERATOR(pg_catalog.=) t.relowner AND j.relowner OPERATOR(pg_catalog.=) p.proowner AND j.relkind OPERATOR(pg_catalog.=) 'r'
          AND pg_catalog.position(tr.tgargs, '\\x00'::pg_catalog.bytea) OPERATOR(pg_catalog.>) 1
          AND args.entry_arguments OPERATOR(pg_catalog.=) (pg_catalog.convert_to(tn.nspname::pg_catalog.text, 'UTF8') OPERATOR(pg_catalog.||) '\\x00'::pg_catalog.bytea
              OPERATOR(pg_catalog.||) pg_catalog.convert_to(t.relname::pg_catalog.text, 'UTF8') OPERATOR(pg_catalog.||) '\\x00'::pg_catalog.bytea OPERATOR(pg_catalog.||) ?)
          AND NOT (args.journal_schema OPERATOR(pg_catalog.~~) 'pg\\_temp%')
          AND NOT (args.journal_schema OPERATOR(pg_catalog.~~) 'pg\\_toast\\_temp%')
          AND (? OR tn.nspname::pg_catalog.text OPERATOR(pg_catalog.=) args.journal_schema)
          AND EXISTS (
              SELECT 1 FROM graphql_orm.change_journal_tables r
              WHERE r.journal_schema OPERATOR(pg_catalog.=) args.journal_schema AND r.declared_table OPERATOR(pg_catalog.=) ?
                AND r.entity_name OPERATOR(pg_catalog.=) ? AND r.key_column OPERATOR(pg_catalog.=) ? AND r.relid::pg_catalog.oid OPERATOR(pg_catalog.=) tr.tgrelid
          )
          AND {REGISTRY_INTACT}
          AND NOT EXISTS (
              SELECT 1 FROM pg_catalog.pg_trigger other
              WHERE other.tgrelid OPERATOR(pg_catalog.=) tr.tgrelid AND other.tgfoid OPERATOR(pg_catalog.=) tr.tgfoid AND other.oid OPERATOR(pg_catalog.<>) tr.oid
          )
          AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_trigger jt WHERE jt.tgrelid OPERATOR(pg_catalog.=) j.oid AND NOT jt.tgisinternal)
          AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_rewrite jr WHERE jr.ev_class OPERATOR(pg_catalog.=) j.oid)
          AND {RECORDER_PRIVILEGES_CLEAN}
          AND pg_catalog.current_setting(?, true) OPERATOR(pg_catalog.=) 'on'
          AND pg_catalog.current_setting('session_replication_role') OPERATOR(pg_catalog.<>) 'replica'"
    );
    let values = vec![
        SqlValue::String(CHANGE_JOURNAL_TABLE.to_string()),
        SqlValue::String(table.clone()),
        SqlValue::String(orm::rls::CHANGE_JOURNAL_TRIGGER.to_string()),
        SqlValue::Int(i64::from(orm::rls::CHANGE_JOURNAL_TRIGGER_TYPE)),
        SqlValue::String(orm::rls::CHANGE_JOURNAL_FUNCTION.to_string()),
        SqlValue::String(orm::rls::CHANGE_JOURNAL_FUNCTION_BODY.to_string()),
        SqlValue::String(orm::journal_key::recorder_body()),
        SqlValue::String(orm::rls::CHANGE_JOURNAL_FUNCTION_CONFIG.to_string()),
        SqlValue::Bytes(expected_arguments),
        SqlValue::Bool(qualified),
        SqlValue::String(metadata.table_name.to_string()),
        SqlValue::String(metadata.entity_name.to_string()),
        SqlValue::String(key.to_string()),
        SqlValue::String(orm::rls::CHANGE_JOURNAL_SETTING.to_string()),
    ];
    if !hook_ctx.fetch_rows(&sql, &values).await?.is_empty() {
        return Ok(());
    }
    Err(sqlx::Error::Protocol(
        diagnose_postgres_change_journal_recorder(hook_ctx, metadata.table_name).await?,
    ))
}

/// Describe why the registry cannot be read by this transaction, if it cannot.
async fn probe_postgres_change_journal_registry<B>(
    hook_ctx: &mut orm::MutationContext<'_, B>,
) -> crate::Result<Option<String>>
where
    B: orm::WriteBackend,
{
    if hook_ctx
        .fetch_rows(
            "SELECT 1 WHERE pg_catalog.to_regnamespace('graphql_orm') IS NOT NULL
               AND pg_catalog.has_schema_privilege(pg_catalog.to_regnamespace('graphql_orm')::pg_catalog.oid, 'USAGE')",
            &[],
        )
        .await?
        .is_empty()
    {
        return Ok(Some(
            "the current role has no USAGE on schema graphql_orm, or it does not exist; apply the schema target or Database::ensure_change_journal_recorder as the schema owner, which grants USAGE to PUBLIC".to_string(),
        ));
    }
    if hook_ctx
        .fetch_rows(
            "SELECT 1 WHERE pg_catalog.to_regclass('graphql_orm.change_journal_tables') IS NOT NULL",
            &[],
        )
        .await?
        .is_empty()
    {
        return Ok(Some(format!(
            "the change journal registry {} does not exist; apply the schema target or Database::ensure_change_journal_recorder as the schema owner",
            orm::rls::CHANGE_JOURNAL_REGISTRY
        )));
    }
    Ok(None)
}

/// Name the first reason the recorder check failed for `table_name`.
async fn diagnose_postgres_change_journal_recorder<B>(
    hook_ctx: &mut orm::MutationContext<'_, B>,
    table_name: &str,
) -> crate::Result<String>
where
    B: orm::WriteBackend,
{
    let setting = vec![SqlValue::String(
        orm::rls::CHANGE_JOURNAL_SETTING.to_string(),
    )];
    if hook_ctx
        .fetch_rows(
            "SELECT 1 WHERE pg_catalog.current_setting(?, true) OPERATOR(pg_catalog.=) 'on'",
            &setting,
        )
        .await?
        .is_empty()
    {
        return Ok(format!(
            "change journal recording is not enabled in this transaction ({} is not 'on'), so writes to {table_name} would not be journaled",
            orm::rls::CHANGE_JOURNAL_SETTING
        ));
    }
    if hook_ctx
        .fetch_rows(
            "SELECT 1 WHERE pg_catalog.current_setting('session_replication_role') OPERATOR(pg_catalog.<>) 'replica'",
            &[],
        )
        .await?
        .is_empty()
    {
        return Ok(format!(
            "session_replication_role is replica, so the change journal trigger on {table_name} would not fire"
        ));
    }
    // Resolved through the session search_path, as the journal itself is.
    let journal = [
        SqlValue::String(format!("\"{CHANGE_JOURNAL_TABLE}\"")),
        SqlValue::String(orm::rls::CHANGE_JOURNAL_FUNCTION.to_string()),
    ];
    if !hook_ctx
        .fetch_rows(
            "SELECT 1 FROM pg_catalog.pg_class j WHERE j.oid OPERATOR(pg_catalog.=) pg_catalog.to_regclass(?)
               AND (EXISTS (SELECT 1 FROM pg_catalog.pg_trigger jt WHERE jt.tgrelid OPERATOR(pg_catalog.=) j.oid AND NOT jt.tgisinternal)
                    OR EXISTS (SELECT 1 FROM pg_catalog.pg_rewrite jr WHERE jr.ev_class OPERATOR(pg_catalog.=) j.oid))",
            &journal[..1],
        )
        .await?
        .is_empty()
    {
        return Ok(format!(
            "the change journal table carries a trigger or rule, which would run as its owner; remove it before journaling writes to {table_name}"
        ));
    }
    let privileges = format!(
        "SELECT 1 FROM pg_catalog.pg_class j
         JOIN pg_catalog.pg_proc p
           ON p.oid OPERATOR(pg_catalog.=) pg_catalog.to_regprocedure(pg_catalog.format('graphql_orm.%I()', ?::pg_catalog.text))
         WHERE j.oid OPERATOR(pg_catalog.=) pg_catalog.to_regclass(?) AND NOT {RECORDER_PRIVILEGES_CLEAN}"
    );
    let privilege_values = vec![journal[1].clone(), journal[0].clone()];
    if !hook_ctx
        .fetch_rows(&privileges, &privilege_values)
        .await?
        .is_empty()
    {
        return Ok(format!(
            "the current role holds a privilege other than SELECT on the change journal, or EXECUTE on graphql_orm.{}(); revoke it before journaling writes to {table_name}",
            orm::rls::CHANGE_JOURNAL_FUNCTION
        ));
    }
    let registry_integrity = format!(
        "SELECT 1 FROM pg_catalog.pg_proc p
         WHERE p.oid OPERATOR(pg_catalog.=) pg_catalog.to_regprocedure(pg_catalog.format('graphql_orm.%I()', ?::pg_catalog.text))
           AND {REGISTRY_INTACT}"
    );
    if hook_ctx
        .fetch_rows(&registry_integrity, &journal[1..])
        .await?
        .is_empty()
    {
        return Ok(format!(
            "the change journal registry {} is not owned by the recorder's owner, or a role other than its owner holds a privilege on it beyond SELECT; revoke it, or apply the schema target, before journaling writes to {table_name}",
            orm::rls::CHANGE_JOURNAL_REGISTRY
        ));
    }
    let registry = vec![SqlValue::String(table_name.to_string())];
    if hook_ctx
        .fetch_rows(
            "SELECT 1 FROM pg_catalog.pg_class g
             WHERE g.oid OPERATOR(pg_catalog.=) pg_catalog.to_regclass('graphql_orm.change_journal_tables')
               AND EXISTS (SELECT 1 FROM graphql_orm.change_journal_tables r WHERE r.declared_table OPERATOR(pg_catalog.=) ?)",
            &registry,
        )
        .await?
        .is_empty()
    {
        return Ok(format!(
            "the change journal registry has no binding for table {table_name}; apply the schema target or Database::ensure_change_journal_recorder as the schema owner"
        ));
    }
    Ok(format!(
        "the change journal recorder, or its registry binding, is missing or altered on table {table_name}; apply the schema target or Database::ensure_change_journal_recorder as the schema owner"
    ))
}
