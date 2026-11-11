use super::core::{
    RlsEntityMetadata, RlsOperation, RlsOperationPolicy, RlsPolicyPlan, RlsSchemaModel,
    SchemaDiagnostic, SchemaDiagnosticKind, SchemaDiagnosticSeverity, SchemaPolicy, SchemaTarget,
};
use super::dialect::{DatabaseBackend, SqlDialect};

/// Render deterministic PostgreSQL helper function SQL for RLS predicates.
///
/// The helpers read transaction-local `app.*` settings set from
/// [`DbAuthContext`](crate::graphql::orm::DbAuthContext).
pub fn postgres_rls_helper_sql() -> Vec<String> {
    vec![
        "CREATE SCHEMA IF NOT EXISTS graphql_orm".to_string(),
        "CREATE OR REPLACE FUNCTION graphql_orm.current_user_id() RETURNS text LANGUAGE sql STABLE AS $$ SELECT NULLIF(current_setting('app.user_id', true), '') $$".to_string(),
        "CREATE OR REPLACE FUNCTION graphql_orm.current_subject() RETURNS text LANGUAGE sql STABLE AS $$ SELECT NULLIF(current_setting('app.subject', true), '') $$".to_string(),
        "CREATE OR REPLACE FUNCTION graphql_orm.current_tenant_id() RETURNS text LANGUAGE sql STABLE AS $$ SELECT NULLIF(current_setting('app.tenant_id', true), '') $$".to_string(),
        "CREATE OR REPLACE FUNCTION graphql_orm.current_roles() RETURNS text[] LANGUAGE sql STABLE AS $$ SELECT ARRAY(SELECT jsonb_array_elements_text(COALESCE(NULLIF(current_setting('app.roles', true), '')::jsonb, '[]'::jsonb))) $$".to_string(),
        "CREATE OR REPLACE FUNCTION graphql_orm.current_scopes() RETURNS text[] LANGUAGE sql STABLE AS $$ SELECT ARRAY(SELECT jsonb_array_elements_text(COALESCE(NULLIF(current_setting('app.scopes', true), '')::jsonb, '[]'::jsonb))) $$".to_string(),
        "CREATE OR REPLACE FUNCTION graphql_orm.claims() RETURNS jsonb LANGUAGE sql STABLE AS $$ SELECT COALESCE(NULLIF(current_setting('app.claims', true), '')::jsonb, '{}'::jsonb) $$".to_string(),
        "CREATE OR REPLACE FUNCTION graphql_orm.has_scope(scope text) RETURNS boolean LANGUAGE sql STABLE AS $$ SELECT scope = ANY(graphql_orm.current_scopes()) $$".to_string(),
    ]
}

/// Fail closed when a generated GraphQL resolver reaches a PostgreSQL RLS
/// entity without request database authorization.
///
/// Generated resolvers call this before database work for every entity they
/// read or write. For a PostgreSQL entity with `#[graphql_rls]` metadata under
/// [`AuthorizationMode::DeclaredPoliciesRequired`] or stricter, the request
/// must carry a [`DbAuthContext`](super::DbAuthContext), or the named trusted
/// bypass [`SystemAccess`](crate::graphql::auth::SystemAccess). Otherwise this
/// returns an `AUTHORIZATION_MISCONFIGURED` error rather than running the query
/// without `app.*` settings. Non-RLS entities, other backends and
/// [`AuthorizationMode::LegacyPermissive`] are unaffected.
///
/// [`AuthorizationMode::DeclaredPoliciesRequired`]: crate::graphql::auth::AuthorizationMode::DeclaredPoliciesRequired
/// [`AuthorizationMode::LegacyPermissive`]: crate::graphql::auth::AuthorizationMode::LegacyPermissive
pub fn ensure_rls_request_context<T, B>(
    ctx: &async_graphql::Context<'_>,
    entity_name: &'static str,
) -> async_graphql::Result<()>
where
    T: super::DatabaseRls,
    B: super::OrmBackend,
{
    if B::DIALECT != DatabaseBackend::Postgres || T::rls_metadata().is_none() {
        return Ok(());
    }
    let mode = ctx
        .data_opt::<crate::db::Database<B>>()
        .map(crate::db::Database::authorization_mode)
        .unwrap_or_default();
    if mode == crate::graphql::auth::AuthorizationMode::LegacyPermissive
        || ctx.data_opt::<super::DbAuthContext>().is_some()
        || ctx
            .data_opt::<crate::graphql::auth::SystemAccess>()
            .is_some()
    {
        return Ok(());
    }
    Err(crate::graphql::errors::OrmPublicError::new(
        crate::graphql::errors::OrmErrorCode::AuthorizationMisconfigured,
    )
    .with_internal(format!(
        "entity {entity_name} declares PostgreSQL row-level security but the request carries no DbAuthContext"
    ))
    .into_graphql_error())
}

/// Build the RLS portion of a full schema target plan.
///
/// Statements are produced only for PostgreSQL under `Managed` or `PlanOnly`.
/// External policies and non-Postgres backends intentionally produce an empty
/// plan.
pub fn build_rls_policy_plan(
    backend: DatabaseBackend,
    policy: SchemaPolicy,
    target: &RlsSchemaModel,
) -> RlsPolicyPlan {
    let include_rls = backend == DatabaseBackend::Postgres
        && matches!(policy, SchemaPolicy::Managed | SchemaPolicy::PlanOnly)
        && !target.entities.is_empty();
    let mut statements = Vec::new();

    if include_rls {
        statements.extend(postgres_rls_helper_sql());
        for entity in sorted_entities(target) {
            statements.extend(render_postgres_entity_rls_sql(entity));
        }
    }

    RlsPolicyPlan {
        backend: backend.name(),
        target_rls_hash: target.stable_hash(),
        statements,
    }
}

/// Build the complete RLS portion of a full schema target plan.
///
/// In addition to [`build_rls_policy_plan`], this covers the ORM-managed
/// tables that hold tenant data on behalf of RLS entities:
///
/// - each RLS entity's PostgreSQL search document table
///   (`__graphql_orm_search_<table>`) gets `ENABLE`, `FORCE` (when the entity
///   forces RLS) and per-command policies that expose a document only while
///   its base row is visible, plus a `BEFORE DELETE` trigger on the base table
///   that removes the document while the base row is still visible;
/// - an existing `__graphql_orm_change_log` gets RLS enabled with no
///   policies, so a role subject to RLS can neither read nor write it.
///
/// With the `change-journal` feature, the plan also creates the change
/// journal and its database-side recorder: a `SECURITY DEFINER` trigger
/// function and an `AFTER INSERT OR UPDATE OR DELETE` row trigger on every
/// single-key table in the target. Each trigger records the journal's
/// schema when it is created.
///
/// Statements are produced only for PostgreSQL under `Managed` or `PlanOnly`,
/// when the target declares at least one RLS entity or the `change-journal`
/// feature is enabled.
pub fn build_schema_target_rls_plan(
    backend: DatabaseBackend,
    policy: SchemaPolicy,
    target: &SchemaTarget,
) -> RlsPolicyPlan {
    let mut plan = build_rls_policy_plan(backend, policy, &target.rls);
    let journal = (cfg!(feature = "change-journal")
        && backend == DatabaseBackend::Postgres
        && matches!(policy, SchemaPolicy::Managed | SchemaPolicy::PlanOnly))
    .then(|| change_journal_tables(&target.schema));
    if !plan.statements.is_empty() {
        let search_tables = rls_search_tables(target);
        if !search_tables.is_empty() {
            plan.statements.push(postgres_search_delete_function_sql());
            for search in &search_tables {
                plan.statements
                    .extend(render_postgres_search_table_rls_sql(search));
            }
        }
        if journal.is_none() {
            plan.statements.push(postgres_change_log_rls_sql());
        }
    }
    if let Some(tables) = journal {
        plan.statements
            .extend(postgres_change_journal_recorder_sql(&tables));
    }
    plan
}

/// Name of the ORM-managed change journal table.
pub(crate) const CHANGE_LOG_TABLE: &str = "__graphql_orm_change_log";

/// Trigger name used on every RLS base table with a search index.
pub(crate) const SEARCH_DELETE_TRIGGER: &str = "graphql_orm_search_delete";

/// Trigger function that removes a base row's search document.
pub(crate) const SEARCH_DELETE_FUNCTION: &str = "delete_search_document";

/// One RLS entity whose PostgreSQL search documents need row security.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RlsSearchTable {
    /// Base entity table path, as declared on the entity.
    pub(crate) base_table: String,
    /// Unquoted ORM search table name.
    pub(crate) search_table: String,
    /// Base primary-key column.
    pub(crate) primary_key: String,
    /// Declared base primary-key SQL type.
    pub(crate) primary_key_sql_type: String,
    /// Whether the base entity forces RLS on its owner.
    pub(crate) force: bool,
}

pub(crate) fn rls_search_tables(target: &SchemaTarget) -> Vec<RlsSearchTable> {
    let mut tables = Vec::new();
    for entity in sorted_entities(&target.rls) {
        let Some(table) = target.schema.tables.iter().find(|table| {
            table.entity_name == entity.entity_name || table.table_name == entity.table_name
        }) else {
            continue;
        };
        for index in &table.search_indexes {
            if index.strategy != super::core::SearchIndexStrategy::PostgresTsvector {
                continue;
            }
            let primary_key_sql_type = table
                .columns
                .iter()
                .find(|column| column.name == index.primary_key)
                .map(|column| column.sql_type.clone())
                .unwrap_or_default();
            tables.push(RlsSearchTable {
                base_table: entity.table_name.to_string(),
                search_table: super::search::search_table_name(&index.table_name),
                primary_key: index.primary_key.clone(),
                primary_key_sql_type,
                force: entity.force,
            });
        }
    }
    tables.sort_by(|left, right| left.search_table.cmp(&right.search_table));
    tables.dedup_by(|left, right| left.search_table == right.search_table);
    tables
}

/// How a search document stores its base primary key in `entity_pk`.
///
/// Must match the ORM writer: binary keys use [`super::binary_key_id`]
/// (`bin:<lowercase hex>`), every other key its PostgreSQL text form.
pub(crate) fn search_key_encoding(search: &RlsSearchTable) -> &'static str {
    match search
        .primary_key_sql_type
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "bytea" => "bin",
        _ => "text",
    }
}

/// Predicate exposing a search document only while its base row is visible.
///
/// The document key is converted to the base key type so the base primary-key
/// index serves the correlated lookup. The text is written in PostgreSQL's
/// deparsed form so drift validation can compare it with `pg_policies`.
pub(crate) fn render_search_document_predicate(search: &RlsSearchTable) -> String {
    let backend = DatabaseBackend::Postgres;
    let base = backend.quote_identifier_path(&search.base_table);
    let key = format!("b.{}", backend.quote_identifier(&search.primary_key));
    let document_key = format!(
        "{}.entity_pk",
        backend.quote_identifier(&search.search_table)
    );
    let normalized = search.primary_key_sql_type.trim().to_ascii_lowercase();
    let condition = match normalized.as_str() {
        "text" => format!("{key} = {document_key}"),
        "uuid" | "bigint" | "integer" | "smallint" => {
            format!("{key} = ({document_key})::{normalized}")
        }
        "bytea" => format!(
            "{key} = CASE WHEN (\"left\"({document_key}, 4) = 'bin:'::text) THEN decode(substr({document_key}, 5), 'hex'::text) ELSE NULL::bytea END"
        ),
        _ => format!("({key})::text = {document_key}"),
    };
    format!("EXISTS (SELECT 1 FROM {base} b WHERE {condition})")
}

fn render_postgres_search_table_rls_sql(search: &RlsSearchTable) -> Vec<String> {
    let backend = DatabaseBackend::Postgres;
    let table = backend.quote_identifier(&search.search_table);
    let predicate = render_search_document_predicate(search);
    let mut statements = vec![format!("ALTER TABLE {table} ENABLE ROW LEVEL SECURITY")];
    if search.force {
        statements.push(format!("ALTER TABLE {table} FORCE ROW LEVEL SECURITY"));
    } else {
        statements.push(format!("ALTER TABLE {table} NO FORCE ROW LEVEL SECURITY"));
    }
    for operation in [
        RlsOperation::Select,
        RlsOperation::Insert,
        RlsOperation::Update,
        RlsOperation::Delete,
    ] {
        let policy_name = backend.quote_identifier(&policy_name(&search.search_table, operation));
        statements.push(format!("DROP POLICY IF EXISTS {policy_name} ON {table}"));
        statements.push(render_create_policy_statement(
            &policy_name,
            &table,
            operation,
            &predicate,
        ));
    }
    statements.push(render_search_delete_trigger_sql(search));
    statements
}

/// (Re)create the base table's search-document delete trigger.
///
/// The search table is resolved once, here, and recorded by schema and name
/// in the trigger arguments, so the trigger never depends on the deleting
/// session's `search_path` and cannot be redirected to a `pg_temp` table.
fn render_search_delete_trigger_sql(search: &RlsSearchTable) -> String {
    let backend = DatabaseBackend::Postgres;
    let base = backend.quote_identifier_path(&search.base_table);
    let trigger = backend.quote_identifier(SEARCH_DELETE_TRIGGER);
    let search_regclass = sql_string_literal(&backend.quote_identifier(&search.search_table));
    format!(
        "DO $graphql_orm$ DECLARE target pg_catalog.regclass := pg_catalog.to_regclass({search_regclass}); target_schema pg_catalog.text; target_name pg_catalog.text; BEGIN \
         IF target IS NULL THEN RAISE EXCEPTION USING ERRCODE = '42P01', MESSAGE = format('graphql-orm search document table %s does not exist', {search_regclass}); END IF; \
         SELECT n.nspname, c.relname INTO target_schema, target_name FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE c.oid = target; \
         IF target_schema LIKE 'pg\\_temp%' OR target_schema LIKE 'pg\\_toast\\_temp%' THEN RAISE EXCEPTION USING ERRCODE = '42P01', MESSAGE = 'graphql-orm search document table resolves to a temporary table'; END IF; \
         DROP TRIGGER IF EXISTS {trigger} ON {base}; \
         EXECUTE format('CREATE TRIGGER %I BEFORE DELETE ON %s FOR EACH ROW EXECUTE FUNCTION graphql_orm.{SEARCH_DELETE_FUNCTION}(%L, %L, %L, %L)', {trigger_name}, {base_literal}, target_schema, target_name, {primary_key}, {encoding}); \
         END $graphql_orm$",
        trigger_name = sql_string_literal(SEARCH_DELETE_TRIGGER),
        base_literal = sql_string_literal(&base),
        primary_key = sql_string_literal(&search.primary_key),
        encoding = sql_string_literal(search_key_encoding(search)),
    )
}

/// Drop the search-document delete trigger from `base_table`, if present.
pub(crate) fn render_drop_search_delete_trigger_sql(base_table: &str) -> String {
    let backend = DatabaseBackend::Postgres;
    let base = backend.quote_identifier_path(base_table);
    format!(
        "DO $graphql_orm$ BEGIN IF to_regclass({}) IS NOT NULL THEN DROP TRIGGER IF EXISTS {} ON {}; END IF; END $graphql_orm$",
        sql_string_literal(&base),
        backend.quote_identifier(SEARCH_DELETE_TRIGGER),
        base,
    )
}

/// Body of `graphql_orm.delete_search_document()`.
///
/// Arguments: search table schema, search table name, base key column, key
/// encoding (`text` or `bin`). The function runs as the invoker with a pinned
/// `search_path`, so the document delete is subject to the same row security
/// as the base delete; it runs before the base row disappears, while the
/// search policies can still see that row. A missing table raises instead of
/// silently leaving the document behind.
pub(crate) const SEARCH_DELETE_FUNCTION_BODY: &str = " DECLARE document_key pg_catalog.text; BEGIN IF TG_NARGS <> 4 OR TG_OP <> 'DELETE' OR TG_WHEN <> 'BEFORE' OR TG_LEVEL <> 'ROW' THEN RAISE EXCEPTION 'graphql-orm search document trigger is misconfigured'; END IF; IF to_regclass(format('%I.%I', TG_ARGV[0], TG_ARGV[1])) IS NULL THEN RAISE EXCEPTION USING ERRCODE = '42P01', MESSAGE = format('graphql-orm search document table %I.%I is missing', TG_ARGV[0], TG_ARGV[1]); END IF; IF TG_ARGV[3] = 'bin' THEN EXECUTE format('SELECT ''bin:''::pg_catalog.text || pg_catalog.encode(($1).%I, ''hex'')', TG_ARGV[2]) INTO document_key USING OLD; ELSIF TG_ARGV[3] = 'text' THEN EXECUTE format('SELECT ($1).%I::pg_catalog.text', TG_ARGV[2]) INTO document_key USING OLD; ELSE RAISE EXCEPTION 'graphql-orm search document trigger has an unknown key encoding'; END IF; EXECUTE format('DELETE FROM %I.%I WHERE entity_pk = $1', TG_ARGV[0], TG_ARGV[1]) USING document_key; RETURN OLD; END ";

/// `search_path` pinned on `graphql_orm.delete_search_document()`, in the
/// stored `pg_proc.proconfig` form. Listing `pg_temp` last stops PostgreSQL
/// from searching it first for relation and type names; the body also
/// schema-qualifies its types.
pub(crate) const SEARCH_DELETE_FUNCTION_CONFIG: &str = "search_path=pg_catalog, pg_temp";

/// Trigger function removing a deleted base row's search document.
pub fn postgres_search_delete_function_sql() -> String {
    format!(
        "CREATE OR REPLACE FUNCTION graphql_orm.{SEARCH_DELETE_FUNCTION}() RETURNS trigger LANGUAGE plpgsql SECURITY INVOKER SET search_path = pg_catalog, pg_temp AS $graphql_orm${SEARCH_DELETE_FUNCTION_BODY}$graphql_orm$"
    )
}

/// Make an existing change journal readable and writable only by privileged
/// roles.
///
/// RLS is enabled (not forced) with no policies, so a role subject to RLS can
/// neither read nor write entries. The `INSERT` policy that earlier releases
/// created is dropped, and `INSERT`, `UPDATE`, `DELETE` and `TRUNCATE` are
/// revoked from `PUBLIC`. Entries are recorded by the database-side recorder,
/// `graphql_orm.record_change_journal_entry()`, which runs as the journal
/// owner. The owner and `BYPASSRLS` roles read the journal for incremental
/// backup export.
pub fn postgres_change_log_rls_sql() -> String {
    let backend = DatabaseBackend::Postgres;
    let table = backend.quote_identifier(CHANGE_LOG_TABLE);
    let policy = backend.quote_identifier(&legacy_change_log_policy_name());
    format!(
        "DO $graphql_orm$ BEGIN IF to_regclass({regclass}) IS NOT NULL THEN ALTER TABLE {table} ENABLE ROW LEVEL SECURITY; DROP POLICY IF EXISTS {policy} ON {table}; REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON {table} FROM PUBLIC; END IF; END $graphql_orm$",
        regclass = sql_string_literal(&table),
    )
}

/// Name of the `INSERT ... WITH CHECK (true)` journal policy created by
/// graphql-orm 0.120.0 to 0.124.x. Current plans drop it.
pub(crate) fn legacy_change_log_policy_name() -> String {
    policy_name(CHANGE_LOG_TABLE, RlsOperation::Insert)
}

/// Idempotently secure the change journal when it is created or opened.
///
/// Unlike [`postgres_change_log_rls_sql`], this changes nothing that the
/// calling role cannot change: RLS is enabled when it is off, and the earlier
/// releases' `INSERT` policy is dropped only when the caller owns the table.
#[cfg(feature = "postgres")]
pub(crate) fn postgres_ensure_change_log_rls_sql() -> String {
    let backend = DatabaseBackend::Postgres;
    let table = backend.quote_identifier(CHANGE_LOG_TABLE);
    let policy_name = legacy_change_log_policy_name();
    let policy = backend.quote_identifier(&policy_name);
    format!(
        "DO $graphql_orm$ BEGIN IF NOT (SELECT relrowsecurity FROM pg_class WHERE oid = to_regclass({regclass})) THEN ALTER TABLE {table} ENABLE ROW LEVEL SECURITY; END IF; IF EXISTS (SELECT 1 FROM pg_policy p JOIN pg_class c ON c.oid = p.polrelid WHERE p.polrelid = to_regclass({regclass}) AND p.polname = {name} AND pg_has_role(c.relowner, 'USAGE')) THEN DROP POLICY {policy} ON {table}; END IF; END $graphql_orm$",
        regclass = sql_string_literal(&table),
        name = sql_string_literal(&policy_name),
    )
}

/// PostgreSQL DDL for the change journal table.
pub(crate) fn postgres_change_journal_table_sql() -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS {} (
            id UUID PRIMARY KEY,
            entity_name TEXT NOT NULL,
            table_name TEXT NOT NULL,
            primary_key TEXT NOT NULL,
            action TEXT NOT NULL,
            changed_at BIGINT NOT NULL,
            transaction_id TEXT NULL,
            row_hash TEXT NULL,
            actor_id TEXT NULL,
            correlation_id TEXT NULL
        )",
        DatabaseBackend::Postgres.quote_identifier(CHANGE_LOG_TABLE)
    )
}

/// Trigger name used on every table whose changes the journal records.
pub(crate) const CHANGE_JOURNAL_TRIGGER: &str = "graphql_orm_change_journal";

/// Trigger function that records one journal entry per changed row.
pub(crate) const CHANGE_JOURNAL_FUNCTION: &str = "record_change_journal_entry";

/// Transaction-local setting the ORM turns on for its own write transactions.
///
/// It only selects which transactions are recorded. The entity, key and
/// action of every entry come from the trigger and the changed row, so a
/// session that sets it can at most journal its own real changes.
#[cfg_attr(
    not(any(
        all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
        all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
    )),
    allow(dead_code)
)]
pub(crate) const CHANGE_JOURNAL_SETTING: &str = "graphql_orm.change_journal";

/// `tgtype` of an `AFTER INSERT OR UPDATE OR DELETE ... FOR EACH ROW` trigger.
pub(crate) const CHANGE_JOURNAL_TRIGGER_TYPE: i32 = 1 | 4 | 8 | 16;

/// `search_path` pinned on `graphql_orm.record_change_journal_entry()`, in the
/// stored `pg_proc.proconfig` form.
pub(crate) const CHANGE_JOURNAL_FUNCTION_CONFIG: &str = SEARCH_DELETE_FUNCTION_CONFIG;

/// Body of `graphql_orm.record_change_journal_entry()`.
///
/// Arguments: journal schema, entity name, table name, key column, key
/// encoding (`text` or `bin`). The function runs as its owner, the journal
/// owner, with a pinned `search_path` and schema-qualified names. It records
/// only while [`CHANGE_JOURNAL_SETTING`] is `on`, refuses to run for a table
/// its owner does not own (such as a session's temporary table), and derives
/// each entry's key and action from the changed row and `TG_OP`. An update
/// that changes the key records a `delete` of the old key and a `create` of
/// the new one.
pub(crate) const CHANGE_JOURNAL_FUNCTION_BODY: &str = " DECLARE entry_key pg_catalog.text; previous_key pg_catalog.text; key_sql pg_catalog.text; entry_sql pg_catalog.text; changed_at pg_catalog.int8; BEGIN IF TG_NARGS <> 5 OR TG_WHEN <> 'AFTER' OR TG_LEVEL <> 'ROW' THEN RAISE EXCEPTION 'graphql-orm change journal trigger is misconfigured'; END IF; IF pg_catalog.current_setting('graphql_orm.change_journal', true) IS DISTINCT FROM 'on' THEN RETURN NULL; END IF; IF TG_ARGV[0] LIKE 'pg\\_temp%' OR TG_ARGV[0] LIKE 'pg\\_toast\\_temp%' OR NOT EXISTS (SELECT 1 FROM pg_catalog.pg_class c WHERE c.oid = TG_RELID AND c.relpersistence <> 't' AND pg_catalog.pg_get_userbyid(c.relowner) = CURRENT_USER) THEN RAISE EXCEPTION 'graphql-orm change journal trigger is attached to a table its recorder does not own'; END IF; IF TG_ARGV[4] = 'bin' THEN key_sql := 'SELECT ''bin:''::pg_catalog.text || pg_catalog.encode(($1).%I, ''hex'')'; ELSIF TG_ARGV[4] = 'text' THEN key_sql := 'SELECT ($1).%I::pg_catalog.text'; ELSE RAISE EXCEPTION 'graphql-orm change journal trigger has an unknown key encoding'; END IF; IF TG_OP <> 'DELETE' THEN EXECUTE pg_catalog.format(key_sql, TG_ARGV[3]) INTO entry_key USING NEW; END IF; IF TG_OP <> 'INSERT' THEN EXECUTE pg_catalog.format(key_sql, TG_ARGV[3]) INTO previous_key USING OLD; END IF; changed_at := pg_catalog.floor(pg_catalog.date_part('epoch', pg_catalog.clock_timestamp()))::pg_catalog.int8; entry_sql := pg_catalog.format('INSERT INTO %I.%I (id, entity_name, table_name, primary_key, action, changed_at) VALUES (pg_catalog.gen_random_uuid(), $1, $2, $3, $4, $5)', TG_ARGV[0], '__graphql_orm_change_log'); IF TG_OP = 'UPDATE' AND previous_key IS NOT DISTINCT FROM entry_key THEN EXECUTE entry_sql USING TG_ARGV[1], TG_ARGV[2], entry_key, 'update'::pg_catalog.text, changed_at; ELSE IF TG_OP <> 'INSERT' THEN EXECUTE entry_sql USING TG_ARGV[1], TG_ARGV[2], previous_key, 'delete'::pg_catalog.text, changed_at; END IF; IF TG_OP <> 'DELETE' THEN EXECUTE entry_sql USING TG_ARGV[1], TG_ARGV[2], entry_key, 'create'::pg_catalog.text, changed_at; END IF; END IF; RETURN NULL; END ";

/// One table whose changes the PostgreSQL change journal records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChangeJournalTable {
    pub(crate) entity_name: String,
    /// Table path, as declared on the entity.
    pub(crate) table_name: String,
    /// Single primary-key column.
    pub(crate) primary_key: String,
    /// `bin` for binary keys, otherwise `text`; matches the ORM event id.
    pub(crate) encoding: &'static str,
}

impl ChangeJournalTable {
    /// Trigger arguments after the journal schema, in order.
    pub(crate) fn trigger_arguments(&self) -> [&str; 4] {
        [
            self.entity_name.as_str(),
            self.table_name.as_str(),
            self.primary_key.as_str(),
            self.encoding,
        ]
    }
}

/// Key encoding for a primary-key SQL type, matching the ORM event id:
/// binary keys use [`super::binary_key_id`], every other key its text form.
pub(crate) fn change_journal_key_encoding(sql_type: &str) -> &'static str {
    match sql_type.trim().to_ascii_lowercase().as_str() {
        "bytea" => "bin",
        _ => "text",
    }
}

/// Tables the journal records: every single-key table in `schema`.
///
/// Composite-key tables are left out; the ORM refuses to journal them.
pub(crate) fn change_journal_tables(schema: &super::core::SchemaModel) -> Vec<ChangeJournalTable> {
    let mut tables = schema
        .tables
        .iter()
        .filter_map(|table| {
            let [primary_key] = table.primary_keys() else {
                return None;
            };
            let sql_type = table
                .columns
                .iter()
                .find(|column| &column.name == primary_key)
                .map(|column| column.sql_type.as_str())
                .unwrap_or_default();
            Some(ChangeJournalTable {
                entity_name: table.entity_name.clone(),
                table_name: table.table_name.clone(),
                primary_key: primary_key.clone(),
                encoding: change_journal_key_encoding(sql_type),
            })
        })
        .collect::<Vec<_>>();
    tables.sort_by(|left, right| left.table_name.cmp(&right.table_name));
    tables.dedup_by(|left, right| left.table_name == right.table_name);
    tables
}

/// Create the PostgreSQL change journal and its database-side recorder.
///
/// The statements create and secure `__graphql_orm_change_log` (see
/// [`postgres_change_log_rls_sql`]), create
/// `graphql_orm.record_change_journal_entry()` as `SECURITY DEFINER` with
/// `search_path = pg_catalog, pg_temp`, revoke its `EXECUTE` from `PUBLIC`,
/// and (re)create an `AFTER INSERT OR UPDATE OR DELETE` row trigger named
/// `graphql_orm_change_journal` on each table. Each trigger records the
/// journal's schema, resolved once when it is created, so neither a session's
/// `search_path` nor a temporary table of the same name can redirect entries.
///
/// Run them as the role that owns the journal and the tables. The recorder
/// runs as that owner, so the runtime role needs no privilege on the journal.
pub(crate) fn postgres_change_journal_recorder_sql(tables: &[ChangeJournalTable]) -> Vec<String> {
    let mut statements = vec![
        "CREATE SCHEMA IF NOT EXISTS graphql_orm".to_string(),
        postgres_change_journal_table_sql(),
        postgres_change_log_rls_sql(),
        format!(
            "CREATE OR REPLACE FUNCTION graphql_orm.{CHANGE_JOURNAL_FUNCTION}() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, pg_temp AS $graphql_orm${CHANGE_JOURNAL_FUNCTION_BODY}$graphql_orm$"
        ),
        format!("REVOKE ALL ON FUNCTION graphql_orm.{CHANGE_JOURNAL_FUNCTION}() FROM PUBLIC"),
    ];
    statements.extend(tables.iter().map(render_change_journal_trigger_sql));
    statements
}

/// (Re)create one table's change journal trigger.
fn render_change_journal_trigger_sql(table: &ChangeJournalTable) -> String {
    let backend = DatabaseBackend::Postgres;
    let base = backend.quote_identifier_path(&table.table_name);
    let journal_regclass = sql_string_literal(&backend.quote_identifier(CHANGE_LOG_TABLE));
    let trigger = backend.quote_identifier(CHANGE_JOURNAL_TRIGGER);
    let [entity, table_name, key, encoding] = table.trigger_arguments().map(sql_string_literal);
    format!(
        "DO $graphql_orm$ DECLARE journal pg_catalog.regclass := pg_catalog.to_regclass({journal_regclass}); journal_schema pg_catalog.text; BEGIN \
         IF journal IS NULL THEN RAISE EXCEPTION USING ERRCODE = '42P01', MESSAGE = 'graphql-orm change journal table does not exist'; END IF; \
         SELECT n.nspname INTO journal_schema FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE c.oid = journal; \
         IF journal_schema LIKE 'pg\\_temp%' OR journal_schema LIKE 'pg\\_toast\\_temp%' THEN RAISE EXCEPTION USING ERRCODE = '42P01', MESSAGE = 'graphql-orm change journal table resolves to a temporary table'; END IF; \
         DROP TRIGGER IF EXISTS {trigger} ON {base}; \
         EXECUTE format('CREATE TRIGGER %I AFTER INSERT OR UPDATE OR DELETE ON %s FOR EACH ROW EXECUTE FUNCTION graphql_orm.{CHANGE_JOURNAL_FUNCTION}(%L, %L, %L, %L, %L)', {trigger_name}, {base_literal}, journal_schema, {entity}, {table_name}, {key}, {encoding}); \
         END $graphql_orm$",
        trigger_name = sql_string_literal(CHANGE_JOURNAL_TRIGGER),
        base_literal = sql_string_literal(&base),
    )
}

fn sql_string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Render deterministic RLS statements for one PostgreSQL entity.
///
/// Output includes `ENABLE ROW LEVEL SECURITY`, optional `FORCE ROW LEVEL
/// SECURITY`, and create-or-replace policy statements for operations with a
/// configured predicate.
pub fn render_postgres_entity_rls_sql(entity: &RlsEntityMetadata) -> Vec<String> {
    let table = DatabaseBackend::Postgres.render_identifier_path(entity.table_name);
    let mut statements = vec![format!("ALTER TABLE {table} ENABLE ROW LEVEL SECURITY")];
    if entity.force {
        statements.push(format!("ALTER TABLE {table} FORCE ROW LEVEL SECURITY"));
    }

    for policy in sorted_policies(entity) {
        let Some(predicate) = render_policy_predicate(DatabaseBackend::Postgres, policy) else {
            continue;
        };
        let policy_name = DatabaseBackend::Postgres
            .quote_identifier(&policy_name(entity.table_name, policy.operation));
        statements.push(format!(
            "DROP POLICY IF EXISTS {} ON {}",
            policy_name, table
        ));
        statements.push(render_create_policy_statement(
            &policy_name,
            &table,
            policy.operation,
            &predicate,
        ));
    }

    statements
}

/// Render the SQL predicate for one operation policy.
///
/// Custom predicates are returned exactly. Generated predicates combine scope,
/// tenant, and owner conditions in deterministic order. Returns `None` when the
/// operation intentionally has no policy.
pub fn render_policy_predicate(
    backend: DatabaseBackend,
    policy: &RlsOperationPolicy,
) -> Option<String> {
    if let Some(predicate) = policy.predicate {
        return Some(predicate.to_string());
    }

    let mut conditions = Vec::new();
    if let Some(scope) = policy.scope {
        conditions.push(format!(
            "graphql_orm.has_scope('{}')",
            scope.replace('\'', "''")
        ));
    }
    if let Some(column) = policy.tenant_column {
        conditions.push(format!(
            "{} = graphql_orm.current_tenant_id()",
            backend.quote_identifier_path(column)
        ));
    }
    if let Some(column) = policy.owner_column {
        conditions.push(format!(
            "{} = graphql_orm.current_user_id()",
            backend.quote_identifier_path(column)
        ));
    }

    if conditions.is_empty() {
        None
    } else {
        Some(conditions.join(" AND "))
    }
}

fn render_create_policy_statement(
    policy_name: &str,
    table_name: &str,
    operation: RlsOperation,
    predicate: &str,
) -> String {
    match operation {
        RlsOperation::Select => format!(
            "CREATE POLICY {} ON {} FOR SELECT USING ({})",
            policy_name, table_name, predicate
        ),
        RlsOperation::Insert => format!(
            "CREATE POLICY {} ON {} FOR INSERT WITH CHECK ({})",
            policy_name, table_name, predicate
        ),
        RlsOperation::Update => format!(
            "CREATE POLICY {} ON {} FOR UPDATE USING ({}) WITH CHECK ({})",
            policy_name, table_name, predicate, predicate
        ),
        RlsOperation::Delete => format!(
            "CREATE POLICY {} ON {} FOR DELETE USING ({})",
            policy_name, table_name, predicate
        ),
    }
}

fn sorted_entities(target: &RlsSchemaModel) -> Vec<&RlsEntityMetadata> {
    let mut entities = target.entities.iter().collect::<Vec<_>>();
    entities.sort_by(|left, right| left.table_name.cmp(right.table_name));
    entities
}

fn sorted_policies(entity: &RlsEntityMetadata) -> Vec<&RlsOperationPolicy> {
    let mut policies = entity.policies.iter().collect::<Vec<_>>();
    policies.sort_by(|left, right| left.operation.cmp(&right.operation));
    policies
}

/// Return the deterministic generated policy name for a table operation.
///
/// Names use the `graphql_orm_<table>_<operation>` shape and are sanitized and
/// shortened deterministically to fit PostgreSQL's identifier length limit.
pub fn policy_name(table_name: &str, operation: RlsOperation) -> String {
    let mut sanitized = String::new();
    for ch in unquote_identifier_path(table_name).chars() {
        if ch.is_ascii_alphanumeric() {
            sanitized.push(ch.to_ascii_lowercase());
        } else if !sanitized.ends_with('_') {
            sanitized.push('_');
        }
    }
    let sanitized = sanitized.trim_matches('_');
    let full = format!("graphql_orm_{}_{}", sanitized, operation.as_str());
    if full.len() <= 63 {
        return full;
    }
    let hash = stable_name_hash(&full);
    format!("{}_{hash}", &full[..54])
}

fn unquote_identifier_path(value: &str) -> String {
    value
        .split('.')
        .map(|part| {
            part.trim()
                .trim_matches('"')
                .trim_matches('[')
                .trim_matches(']')
                .replace("\"\"", "\"")
                .replace("]]", "]")
        })
        .collect::<Vec<_>>()
        .join("_")
}

fn stable_name_hash(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:08x}")
}

#[doc(hidden)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveRlsPolicy {
    pub policy_name: String,
    pub operation: RlsOperation,
    pub using_expression: Option<String>,
    pub check_expression: Option<String>,
}

#[doc(hidden)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveRlsTable {
    pub table_name: String,
    pub enabled: bool,
    pub forced: bool,
    pub policies: Vec<LiveRlsPolicy>,
}

pub(crate) fn validate_rls_models(
    backend: DatabaseBackend,
    policy: SchemaPolicy,
    current: &[LiveRlsTable],
    target: &RlsSchemaModel,
) -> Vec<SchemaDiagnostic> {
    if target.entities.is_empty()
        || backend != DatabaseBackend::Postgres
        || matches!(
            policy,
            SchemaPolicy::ExternalReadOnly | SchemaPolicy::ExternalWritable
        )
    {
        return Vec::new();
    }

    let current = current
        .iter()
        .map(|table| (normalize_table_key(&table.table_name), table))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut diagnostics = Vec::new();

    for entity in sorted_entities(target) {
        let table_key = normalize_table_key(entity.table_name);
        let Some(current_table) = current.get(&table_key) else {
            diagnostics.push(rls_diagnostic(
                SchemaDiagnosticSeverity::Error,
                entity.table_name,
                format!("Missing RLS table state for {}", entity.table_name),
            ));
            continue;
        };
        if !current_table.enabled {
            diagnostics.push(rls_diagnostic(
                SchemaDiagnosticSeverity::Error,
                entity.table_name,
                format!("RLS is not enabled on {}", entity.table_name),
            ));
        }
        if entity.force && !current_table.forced {
            diagnostics.push(rls_diagnostic(
                SchemaDiagnosticSeverity::Error,
                entity.table_name,
                format!("RLS is not forced on {}", entity.table_name),
            ));
        }

        let policies = current_table
            .policies
            .iter()
            .map(|policy| (policy.policy_name.as_str(), policy))
            .collect::<std::collections::BTreeMap<_, _>>();
        for expected in sorted_policies(entity) {
            let Some(predicate) = render_policy_predicate(DatabaseBackend::Postgres, expected)
            else {
                continue;
            };
            let expected_name = policy_name(entity.table_name, expected.operation);
            let Some(current_policy) = policies.get(expected_name.as_str()) else {
                diagnostics.push(rls_diagnostic(
                    SchemaDiagnosticSeverity::Error,
                    entity.table_name,
                    format!(
                        "Missing RLS policy {} on {}",
                        expected_name, entity.table_name
                    ),
                ));
                continue;
            };
            if current_policy.operation != expected.operation {
                diagnostics.push(rls_diagnostic(
                    SchemaDiagnosticSeverity::Error,
                    entity.table_name,
                    format!("RLS policy {} has unexpected operation", expected_name),
                ));
            }
            let expected = normalize_policy_expr(&predicate);
            let using_expr = current_policy
                .using_expression
                .as_deref()
                .map(normalize_policy_expr);
            let check_expr = current_policy
                .check_expression
                .as_deref()
                .map(normalize_policy_expr);
            match current_policy.operation {
                RlsOperation::Select | RlsOperation::Delete => {
                    if using_expr.as_deref() != Some(expected.as_str()) {
                        diagnostics.push(rls_diagnostic(
                            SchemaDiagnosticSeverity::Error,
                            entity.table_name,
                            format!("RLS policy {} USING expression differs", expected_name),
                        ));
                    }
                }
                RlsOperation::Insert => {
                    if check_expr.as_deref() != Some(expected.as_str()) {
                        diagnostics.push(rls_diagnostic(
                            SchemaDiagnosticSeverity::Error,
                            entity.table_name,
                            format!("RLS policy {} WITH CHECK expression differs", expected_name),
                        ));
                    }
                }
                RlsOperation::Update => {
                    if using_expr.as_deref() != Some(expected.as_str())
                        || check_expr.as_deref() != Some(expected.as_str())
                    {
                        diagnostics.push(rls_diagnostic(
                            SchemaDiagnosticSeverity::Error,
                            entity.table_name,
                            format!("RLS policy {} expression differs", expected_name),
                        ));
                    }
                }
            }
        }
    }

    diagnostics
}

/// Live PostgreSQL facts that decide whether RLS is enforced for the current
/// connection role.
#[doc(hidden)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveRlsEnforcement {
    /// `current_user` of the introspecting connection.
    pub role: String,
    /// `pg_roles.rolsuper` for the current role.
    pub superuser: bool,
    /// `pg_roles.rolbypassrls` for the current role.
    pub bypass_rls: bool,
    /// `row_security_active(table)` per table key (same keys as [`LiveRlsTable`]).
    pub row_security_active: std::collections::BTreeMap<String, bool>,
    /// ORM search-document delete triggers, by base table key.
    pub search_delete_triggers: std::collections::BTreeMap<String, LiveSearchDeleteTrigger>,
    /// Live `graphql_orm.delete_search_document()` definition, when present.
    pub search_delete_function: Option<LiveSearchDeleteFunction>,
    /// Every policy name per table key, including `FOR ALL` policies.
    pub policy_names: std::collections::BTreeMap<String, Vec<String>>,
    /// ORM change journal triggers, by table key.
    pub change_journal_triggers: std::collections::BTreeMap<String, LiveChangeJournalTrigger>,
    /// Live `graphql_orm.record_change_journal_entry()` definition, when present.
    pub change_journal_function: Option<LiveChangeJournalFunction>,
}

/// A live trigger named [`CHANGE_JOURNAL_TRIGGER`].
#[doc(hidden)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveChangeJournalTrigger {
    /// `pg_trigger.tgenabled` (`O` fires in ordinary sessions).
    pub enabled: String,
    /// `pg_trigger.tgtype`; 29 is `AFTER INSERT OR UPDATE OR DELETE ... FOR EACH ROW`.
    pub trigger_type: i32,
    /// Whether the trigger has a `WHEN` condition.
    pub conditional: bool,
    /// Whether it executes `graphql_orm.record_change_journal_entry()`.
    pub calls_orm_function: bool,
    /// Whether the function owner also owns the table.
    pub function_owns_table: bool,
    /// Trigger arguments in order.
    pub arguments: Vec<String>,
    /// Whether the schema named by the first argument holds a change journal
    /// owned by the function owner.
    pub journal_owned_by_function_owner: bool,
}

/// The live `graphql_orm.record_change_journal_entry()` definition.
#[doc(hidden)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveChangeJournalFunction {
    /// `pg_proc.prosrc`.
    pub body: String,
    /// `pg_proc.proconfig`.
    pub config: Vec<String>,
    /// `pg_proc.prosecdef`.
    pub security_definer: bool,
}

/// A live trigger named [`SEARCH_DELETE_TRIGGER`].
#[doc(hidden)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveSearchDeleteTrigger {
    /// `pg_trigger.tgenabled` (`O` fires in ordinary sessions).
    pub enabled: String,
    /// `pg_trigger.tgtype`; 11 is `BEFORE DELETE ... FOR EACH ROW`.
    pub trigger_type: i32,
    /// Whether it executes `graphql_orm.delete_search_document()`.
    pub calls_orm_function: bool,
    /// Trigger arguments in order.
    pub arguments: Vec<String>,
    /// Whether the schema-qualified search table named by the arguments exists.
    pub target_exists: bool,
}

/// The live `graphql_orm.delete_search_document()` definition.
#[doc(hidden)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveSearchDeleteFunction {
    /// `pg_proc.prosrc`.
    pub body: String,
    /// `pg_proc.proconfig`.
    pub config: Vec<String>,
    /// `pg_proc.prosecdef`.
    pub security_definer: bool,
}

/// `tgtype` of a `BEFORE DELETE ... FOR EACH ROW` trigger.
pub(crate) const SEARCH_DELETE_TRIGGER_TYPE: i32 = 1 | 2 | 8;

/// Report Error diagnostics when PostgreSQL would not enforce the target's RLS
/// for the current connection role.
///
/// PostgreSQL skips every policy for a superuser or a `BYPASSRLS` role, and
/// for the owner of a table without `FORCE ROW LEVEL SECURITY`. The check
/// covers each RLS entity table, its search document table, and the change
/// journal when it exists.
pub(crate) fn validate_rls_enforcement_state(
    backend: DatabaseBackend,
    live: &LiveRlsEnforcement,
    target: &SchemaTarget,
) -> Vec<SchemaDiagnostic> {
    if backend != DatabaseBackend::Postgres || target.rls.entities.is_empty() {
        return Vec::new();
    }
    let mut diagnostics = Vec::new();
    if live.superuser {
        diagnostics.push(enforcement_diagnostic(
            None,
            format!(
                "Current database role {} is a superuser; PostgreSQL skips every RLS policy, so RLS is not enforced. Connect the runtime pool as a role without SUPERUSER",
                live.role
            ),
        ));
    }
    if live.bypass_rls {
        diagnostics.push(enforcement_diagnostic(
            None,
            format!(
                "Current database role {} has BYPASSRLS; PostgreSQL skips every RLS policy, so RLS is not enforced. Connect the runtime pool as a role with NOBYPASSRLS",
                live.role
            ),
        ));
    }
    if !diagnostics.is_empty() {
        return diagnostics;
    }

    let mut protected = sorted_entities(&target.rls)
        .into_iter()
        .map(|entity| entity.table_name.to_string())
        .collect::<Vec<_>>();
    protected.extend(
        rls_search_tables(target)
            .into_iter()
            .map(|search| search.search_table),
    );
    protected.push(CHANGE_LOG_TABLE.to_string());
    for table in protected {
        if live.row_security_active.get(&normalize_table_key(&table)) == Some(&false) {
            diagnostics.push(enforcement_diagnostic(
                Some(&table),
                format!(
                    "RLS is not enforced on {table} for current database role {}: the role owns the table without FORCE ROW LEVEL SECURITY, or RLS is disabled",
                    live.role
                ),
            ));
        }
    }
    diagnostics
}

fn enforcement_diagnostic(table: Option<&str>, message: String) -> SchemaDiagnostic {
    SchemaDiagnostic {
        severity: SchemaDiagnosticSeverity::Error,
        kind: SchemaDiagnosticKind::RlsMismatch,
        table: table.map(str::to_string),
        column: None,
        message,
    }
}

/// Validate RLS on the ORM-managed search document tables of RLS entities and
/// on the change journal.
pub(crate) fn validate_protected_rls_tables(
    backend: DatabaseBackend,
    policy: SchemaPolicy,
    current: &[LiveRlsTable],
    live: Option<&LiveRlsEnforcement>,
    target: &SchemaTarget,
) -> Vec<SchemaDiagnostic> {
    if target.rls.entities.is_empty()
        || backend != DatabaseBackend::Postgres
        || matches!(
            policy,
            SchemaPolicy::ExternalReadOnly | SchemaPolicy::ExternalWritable
        )
    {
        return Vec::new();
    }
    let current = current
        .iter()
        .map(|table| (normalize_table_key(&table.table_name), table))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut diagnostics = Vec::new();

    for search in rls_search_tables(target) {
        let table_name = search.search_table.as_str();
        let key = normalize_table_key(table_name);
        let Some(current_table) = current.get(&key) else {
            diagnostics.push(rls_diagnostic(
                SchemaDiagnosticSeverity::Error,
                table_name,
                format!("Missing RLS table state for {table_name}"),
            ));
            continue;
        };
        if !current_table.enabled {
            diagnostics.push(rls_diagnostic(
                SchemaDiagnosticSeverity::Error,
                table_name,
                format!("RLS is not enabled on {table_name}"),
            ));
        }
        if search.force && !current_table.forced {
            diagnostics.push(rls_diagnostic(
                SchemaDiagnosticSeverity::Error,
                table_name,
                format!("RLS is not forced on {table_name}"),
            ));
        }
        let predicate = canonical_policy_expr(&render_search_document_predicate(&search));
        let mut expected_names = Vec::new();
        for operation in [
            RlsOperation::Select,
            RlsOperation::Insert,
            RlsOperation::Update,
            RlsOperation::Delete,
        ] {
            let name = policy_name(table_name, operation);
            let Some(live_policy) = current_table
                .policies
                .iter()
                .find(|live_policy| live_policy.policy_name == name)
            else {
                diagnostics.push(rls_diagnostic(
                    SchemaDiagnosticSeverity::Error,
                    table_name,
                    format!("Missing RLS policy {name} on {table_name}"),
                ));
                expected_names.push(name);
                continue;
            };
            let using = live_policy
                .using_expression
                .as_deref()
                .map(canonical_policy_expr);
            let check = live_policy
                .check_expression
                .as_deref()
                .map(canonical_policy_expr);
            let matches = live_policy.operation == operation
                && match operation {
                    RlsOperation::Select | RlsOperation::Delete => {
                        using.as_deref() == Some(predicate.as_str())
                    }
                    RlsOperation::Insert => check.as_deref() == Some(predicate.as_str()),
                    RlsOperation::Update => {
                        using.as_deref() == Some(predicate.as_str())
                            && check.as_deref() == Some(predicate.as_str())
                    }
                };
            if !matches {
                diagnostics.push(rls_diagnostic(
                    SchemaDiagnosticSeverity::Error,
                    table_name,
                    format!("RLS policy {name} on {table_name} does not match the visible base row predicate"),
                ));
            }
            expected_names.push(name);
        }
        if let Some(live) = live {
            report_unexpected_policies(&mut diagnostics, live, table_name, &expected_names);
            if let Some(message) = search_delete_trigger_problem(live, &search) {
                diagnostics.push(rls_diagnostic(
                    SchemaDiagnosticSeverity::Error,
                    table_name,
                    message,
                ));
            }
        }
    }

    diagnostics
}

/// Validate the change journal's row security and, with the
/// `change-journal` feature, its database-side recorder.
///
/// Applies to PostgreSQL under `Managed`, `ValidateOnly` and `PlanOnly` when
/// the target declares an RLS entity or the feature is enabled. The journal
/// must have RLS enabled and no policies: any policy, including the `INSERT`
/// policy of earlier releases, lets a role subject to RLS read or write
/// entries. With the feature, the journal must exist, the recorder function
/// must match the definition graphql-orm installs, and every single-key table
/// in the target must carry an intact recorder trigger.
pub(crate) fn validate_change_journal(
    backend: DatabaseBackend,
    policy: SchemaPolicy,
    current: &[LiveRlsTable],
    live: Option<&LiveRlsEnforcement>,
    target: &SchemaTarget,
) -> Vec<SchemaDiagnostic> {
    let recorder = cfg!(feature = "change-journal");
    if backend != DatabaseBackend::Postgres
        || (!recorder && target.rls.entities.is_empty())
        || matches!(
            policy,
            SchemaPolicy::ExternalReadOnly | SchemaPolicy::ExternalWritable
        )
    {
        return Vec::new();
    }
    let mut diagnostics = Vec::new();
    let error = |message: String| {
        rls_diagnostic(SchemaDiagnosticSeverity::Error, CHANGE_LOG_TABLE, message)
    };
    match current
        .iter()
        .find(|table| normalize_table_key(&table.table_name) == CHANGE_LOG_TABLE)
    {
        Some(change_log) => {
            if !change_log.enabled {
                diagnostics.push(error(format!("RLS is not enabled on {CHANGE_LOG_TABLE}")));
            }
            let mut names = change_log
                .policies
                .iter()
                .map(|policy| policy.policy_name.clone())
                .collect::<Vec<_>>();
            if let Some(live) = live
                && let Some(all) = live.policy_names.get(CHANGE_LOG_TABLE)
            {
                names.extend(all.iter().cloned());
            }
            names.sort();
            names.dedup();
            for name in names {
                diagnostics.push(error(format!(
                    "Unexpected RLS policy {name} on {CHANGE_LOG_TABLE} lets a role subject to RLS read or write journal entries; re-apply the schema target"
                )));
            }
        }
        None if recorder => {
            diagnostics.push(error(format!(
                "Missing change journal table {CHANGE_LOG_TABLE}"
            )));
        }
        None => {}
    }
    if !recorder {
        return diagnostics;
    }
    let Some(live) = live else {
        return diagnostics;
    };
    let function_matches = live
        .change_journal_function
        .as_ref()
        .is_some_and(|function| {
            function.body == CHANGE_JOURNAL_FUNCTION_BODY
                && function.config == [CHANGE_JOURNAL_FUNCTION_CONFIG]
                && function.security_definer
        });
    if !function_matches {
        diagnostics.push(error(format!(
            "graphql_orm.{CHANGE_JOURNAL_FUNCTION}() is missing or differs from the definition graphql-orm installs"
        )));
    }
    for table in change_journal_tables(&target.schema) {
        if let Some(message) = change_journal_trigger_problem(live, &table) {
            diagnostics.push(rls_diagnostic(
                SchemaDiagnosticSeverity::Error,
                &table.table_name,
                message,
            ));
        }
    }
    diagnostics
}

/// Describe why the live change journal trigger for `table` would not record
/// its changes, if it would not.
fn change_journal_trigger_problem(
    live: &LiveRlsEnforcement,
    table: &ChangeJournalTable,
) -> Option<String> {
    let name = &table.table_name;
    let Some(trigger) = live.change_journal_triggers.get(&normalize_table_key(name)) else {
        return Some(format!(
            "Missing change journal trigger {CHANGE_JOURNAL_TRIGGER} on {name}"
        ));
    };
    if trigger.enabled != "O" {
        return Some(format!(
            "Change journal trigger {CHANGE_JOURNAL_TRIGGER} on {name} does not fire in ordinary sessions (tgenabled = {})",
            trigger.enabled
        ));
    }
    if trigger.trigger_type != CHANGE_JOURNAL_TRIGGER_TYPE
        || trigger.conditional
        || !trigger.calls_orm_function
    {
        return Some(format!(
            "Change journal trigger {CHANGE_JOURNAL_TRIGGER} on {name} is not an unconditional AFTER INSERT OR UPDATE OR DELETE row trigger calling graphql_orm.{CHANGE_JOURNAL_FUNCTION}()"
        ));
    }
    let expected = table.trigger_arguments();
    let arguments_match = matches!(
        trigger.arguments.as_slice(),
        [schema, rest @ ..]
            if !schema.starts_with("pg_temp")
                && !schema.starts_with("pg_toast_temp")
                && rest.iter().map(String::as_str).eq(expected)
    );
    if !arguments_match || !trigger.journal_owned_by_function_owner {
        return Some(format!(
            "Change journal trigger {CHANGE_JOURNAL_TRIGGER} on {name} does not record {} by key {} into a {CHANGE_LOG_TABLE} owned by the recorder's owner",
            table.entity_name, table.primary_key
        ));
    }
    if !trigger.function_owns_table {
        return Some(format!(
            "graphql_orm.{CHANGE_JOURNAL_FUNCTION}() is not owned by the owner of {name}; apply the schema target as the role that owns the tables and the journal"
        ));
    }
    None
}

/// Describe why the live search-document delete trigger for `search` would
/// not reliably remove its documents, if it would not.
fn search_delete_trigger_problem(
    live: &LiveRlsEnforcement,
    search: &RlsSearchTable,
) -> Option<String> {
    let base = &search.base_table;
    let Some(trigger) = live.search_delete_triggers.get(&normalize_table_key(base)) else {
        return Some(format!(
            "Missing search document delete trigger {SEARCH_DELETE_TRIGGER} on {base}"
        ));
    };
    if trigger.enabled != "O" {
        return Some(format!(
            "Search document delete trigger {SEARCH_DELETE_TRIGGER} on {base} does not fire in ordinary sessions (tgenabled = {})",
            trigger.enabled
        ));
    }
    if trigger.trigger_type != SEARCH_DELETE_TRIGGER_TYPE || !trigger.calls_orm_function {
        return Some(format!(
            "Search document delete trigger {SEARCH_DELETE_TRIGGER} on {base} is not a BEFORE DELETE row trigger calling graphql_orm.{SEARCH_DELETE_FUNCTION}()"
        ));
    }
    let targets_search_table = matches!(
        trigger.arguments.as_slice(),
        [schema, table, key, encoding]
            if !schema.starts_with("pg_temp")
                && !schema.starts_with("pg_toast_temp")
                && *table == search.search_table
                && *key == search.primary_key
                && encoding == search_key_encoding(search)
    );
    if !targets_search_table || !trigger.target_exists {
        return Some(format!(
            "Search document delete trigger {SEARCH_DELETE_TRIGGER} on {base} does not target {} by key {}",
            search.search_table, search.primary_key
        ));
    }
    let function_matches = live
        .search_delete_function
        .as_ref()
        .is_some_and(|function| {
            function.body == SEARCH_DELETE_FUNCTION_BODY
                && function.config == [SEARCH_DELETE_FUNCTION_CONFIG]
                && !function.security_definer
        });
    if !function_matches {
        return Some(format!(
            "graphql_orm.{SEARCH_DELETE_FUNCTION}() differs from the definition graphql-orm installs"
        ));
    }
    None
}

fn report_unexpected_policies(
    diagnostics: &mut Vec<SchemaDiagnostic>,
    live: &LiveRlsEnforcement,
    table_name: &str,
    expected: &[String],
) {
    let Some(names) = live.policy_names.get(&normalize_table_key(table_name)) else {
        return;
    };
    for name in names {
        if !expected.contains(name) {
            diagnostics.push(rls_diagnostic(
                SchemaDiagnosticSeverity::Error,
                table_name,
                format!(
                    "Unexpected RLS policy {name} on {table_name} widens access to ORM-managed rows"
                ),
            ));
        }
    }
}

/// Canonicalize a policy expression for comparison with PostgreSQL's deparsed
/// `pg_policies` text: case, whitespace, quoting, grouping parentheses and the
/// default `public.` qualification are not significant for the fixed-shape
/// expressions generated for ORM-managed tables.
fn canonical_policy_expr(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_whitespace() && !matches!(ch, '(' | ')' | '"'))
        .collect::<String>()
        .to_ascii_lowercase()
        .replace("public.", "")
}

fn normalize_table_key(table_name: &str) -> String {
    let unquoted = unquote_identifier_path(table_name);
    unquoted
        .strip_prefix("public_")
        .unwrap_or(&unquoted)
        .to_string()
}

fn normalize_policy_expr(value: &str) -> String {
    let mut value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    loop {
        let trimmed = value.trim();
        if trimmed.starts_with('(') && trimmed.ends_with(')') {
            value = trimmed[1..trimmed.len() - 1].trim().to_string();
        } else {
            break;
        }
    }
    value
}

fn rls_diagnostic(
    severity: SchemaDiagnosticSeverity,
    table: &str,
    message: String,
) -> SchemaDiagnostic {
    SchemaDiagnostic {
        severity,
        kind: SchemaDiagnosticKind::RlsMismatch,
        table: Some(table.to_string()),
        column: None,
        message,
    }
}
