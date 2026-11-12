use super::core::{
    RlsEntityMetadata, RlsOperation, RlsOperationPolicy, RlsPolicyPlan, RlsSchemaModel,
    SchemaDiagnostic, SchemaDiagnosticKind, SchemaDiagnosticSeverity, SchemaPolicy, SchemaTarget,
};
use super::dialect::{DatabaseBackend, SqlDialect};

/// One PostgreSQL helper function the RLS plan installs in `graphql_orm`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RlsHelperFunction {
    /// Function name in schema `graphql_orm`.
    pub(crate) name: &'static str,
    /// Arguments, as `pg_get_function_identity_arguments` renders them.
    pub(crate) arguments: &'static str,
    /// Result type, as `pg_get_function_result` renders it.
    pub(crate) returns: &'static str,
    /// `LANGUAGE sql` body, exactly as stored in `pg_proc.prosrc`.
    pub(crate) body: &'static str,
}

/// The helper functions RLS predicates call. Each is `LANGUAGE sql STABLE`,
/// `SECURITY INVOKER`, not `STRICT`, with no `SET` options.
pub(crate) const RLS_HELPER_FUNCTIONS: &[RlsHelperFunction] = &[
    RlsHelperFunction {
        name: "current_user_id",
        arguments: "",
        returns: "text",
        body: " SELECT NULLIF(current_setting('app.user_id', true), '') ",
    },
    RlsHelperFunction {
        name: "current_subject",
        arguments: "",
        returns: "text",
        body: " SELECT NULLIF(current_setting('app.subject', true), '') ",
    },
    RlsHelperFunction {
        name: "current_tenant_id",
        arguments: "",
        returns: "text",
        body: " SELECT NULLIF(current_setting('app.tenant_id', true), '') ",
    },
    RlsHelperFunction {
        name: "current_roles",
        arguments: "",
        returns: "text[]",
        body: " SELECT ARRAY(SELECT jsonb_array_elements_text(COALESCE(NULLIF(current_setting('app.roles', true), '')::jsonb, '[]'::jsonb))) ",
    },
    RlsHelperFunction {
        name: "current_scopes",
        arguments: "",
        returns: "text[]",
        body: " SELECT ARRAY(SELECT jsonb_array_elements_text(COALESCE(NULLIF(current_setting('app.scopes', true), '')::jsonb, '[]'::jsonb))) ",
    },
    RlsHelperFunction {
        name: "claims",
        arguments: "",
        returns: "jsonb",
        body: " SELECT COALESCE(NULLIF(current_setting('app.claims', true), '')::jsonb, '{}'::jsonb) ",
    },
    RlsHelperFunction {
        name: "has_scope",
        arguments: "scope text",
        returns: "boolean",
        body: " SELECT scope = ANY(graphql_orm.current_scopes()) ",
    },
];

/// Render deterministic PostgreSQL helper function SQL for RLS predicates.
///
/// The helpers read transaction-local `app.*` settings set from
/// [`DbAuthContext`](crate::graphql::orm::DbAuthContext).
pub fn postgres_rls_helper_sql() -> Vec<String> {
    let mut statements = vec!["CREATE SCHEMA IF NOT EXISTS graphql_orm".to_string()];
    statements.extend(RLS_HELPER_FUNCTIONS.iter().map(|helper| {
        format!(
            "CREATE OR REPLACE FUNCTION graphql_orm.{}({}) RETURNS {} LANGUAGE sql STABLE AS $${}$$",
            helper.name, helper.arguments, helper.returns, helper.body
        )
    }));
    statements
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
/// index serves the correlated lookup. Drift validation compares
/// PostgreSQL's rendering of this text with its rendering of the live policy.
pub(crate) fn render_search_document_predicate(search: &RlsSearchTable) -> String {
    render_search_document_predicate_on(
        &DatabaseBackend::Postgres.render_identifier_path(&search.base_table),
        &search_document_probe(search),
    )
}

/// [`render_search_document_predicate`] with the base table written as
/// `base_sql`. Validation passes the resolved, schema-qualified base table.
pub(crate) fn render_search_document_predicate_on(
    base_sql: &str,
    search: &RlsSearchDocumentProbe,
) -> String {
    let base = base_sql;
    let key = format!("b.{}", quote_policy_column(&search.primary_key));
    let document_key = format!("{}.entity_pk", search.search_table_sql);
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
    let table = search_table_sql(search);
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
    let base = backend.render_identifier_path(&search.base_table);
    let trigger = backend.quote_identifier(SEARCH_DELETE_TRIGGER);
    let search_regclass = sql_string_literal(&search_table_sql(search));
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
        primary_key = sql_string_literal(&super::dialect::postgres_resolved_identifier(
            &search.primary_key
        )),
        encoding = sql_string_literal(search_key_encoding(search)),
    )
}

/// Drop the search-document delete trigger from `base_table`, if present.
pub(crate) fn render_drop_search_delete_trigger_sql(base_table: &str) -> String {
    let backend = DatabaseBackend::Postgres;
    let base = backend.render_identifier_path(base_table);
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

/// Make an existing change journal readable and writable only by its owner.
///
/// RLS is enabled (not forced) with no policies, so a role subject to RLS can
/// neither read nor write entries. The `INSERT` policy that earlier releases
/// created is dropped, and every table or column privilege other than
/// `SELECT` that any role other than the owner holds on the journal, `PUBLIC`
/// included, is revoked; the statement raises, rolling back the plan, if one
/// survives. Entries are recorded by the
/// database-side recorder, `graphql_orm.record_change_journal_entry()`, which
/// runs as the journal owner, so a role that could add a trigger to the
/// journal could run code as that owner. `SELECT` grants are kept: they cannot
/// write or add triggers, and with no policies they read nothing unless the
/// role also has `BYPASSRLS`, as a dedicated backup export role does.
pub fn postgres_change_log_rls_sql() -> String {
    let backend = DatabaseBackend::Postgres;
    let table = backend.quote_identifier(CHANGE_LOG_TABLE);
    let policy = backend.quote_identifier(&legacy_change_log_policy_name());
    render_secure_relation(
        &sql_string_literal(&table),
        &format!(
            "ALTER TABLE {table} ENABLE ROW LEVEL SECURITY; DROP POLICY IF EXISTS {policy} ON {table};"
        ),
        "change journal",
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

/// Schema-qualified registry binding each journaled declared table to its
/// table identity.
pub(crate) const CHANGE_JOURNAL_REGISTRY: &str = "graphql_orm.change_journal_tables";

/// Body of `graphql_orm.record_change_journal_entry()`.
///
/// Arguments: journal schema, table schema, table name (both resolved when the
/// trigger is created), entity name, declared table name, key column, key
/// encoding (`text` or `bin`). The function runs as its owner, the journal
/// owner, with a pinned `search_path` and schema-qualified names. It records
/// only while [`CHANGE_JOURNAL_SETTING`] is `on`, and raises unless:
///
/// - it fires as an `AFTER` row trigger named [`CHANGE_JOURNAL_TRIGGER`];
/// - the table it fires on is the resolved table in its arguments, is not
///   temporary and is owned by the function owner;
/// - the key column is the table's sole primary-key column;
/// - the registry ([`CHANGE_JOURNAL_REGISTRY`]) is owned by the function
///   owner, grants nothing beyond `SELECT` to other roles, and has a row for
///   this journal schema and declared table, with this entity name and key
///   column, whose table identity is the table the trigger fires on. Plans
///   write that row from the declared path exactly as the ORM's own writes
///   render it, so the binding holds for every naming form;
/// - the journal exists in the recorded schema, is owned by the function
///   owner, carries no non-internal trigger or rule, and grants `TRIGGER` to
///   no other role. The journal is locked first, so no trigger can be added
///   after the check until the transaction ends. Under `REPEATABLE READ` or
///   `SERIALIZABLE` the trigger lookup reads the transaction snapshot and can
///   miss one committed after the snapshot and before the lock; the
///   `TRIGGER` refusal narrows that to a role that held `TRIGGER` then and no
///   longer does, or the owner. Column privileges cannot include `TRIGGER`.
///
/// Each entry's key comes from the changed row and its action from `TG_OP`.
/// An update that changes the key records a `delete` of the old key and a
/// `create` of the new one.
pub(crate) const CHANGE_JOURNAL_FUNCTION_BODY: &str = concat!(
    " DECLARE entry_key pg_catalog.text; previous_key pg_catalog.text; key_sql pg_catalog.text; entry_sql pg_catalog.text; changed_at pg_catalog.int8; journal pg_catalog.oid; BEGIN",
    " IF TG_NARGS <> 7 OR TG_WHEN <> 'AFTER' OR TG_LEVEL <> 'ROW' OR TG_NAME <> 'graphql_orm_change_journal' THEN RAISE EXCEPTION 'graphql-orm change journal trigger is misconfigured'; END IF;",
    " IF pg_catalog.current_setting('graphql_orm.change_journal', true) IS DISTINCT FROM 'on' THEN RETURN NULL; END IF;",
    " IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_class g WHERE g.oid = pg_catalog.to_regclass('graphql_orm.change_journal_tables') AND pg_catalog.pg_get_userbyid(g.relowner) = CURRENT_USER",
    " AND NOT EXISTS (SELECT 1 FROM (SELECT g.relacl AS acl UNION ALL SELECT att.attacl FROM pg_catalog.pg_attribute att WHERE att.attrelid = g.oid AND att.attnum > 0 AND NOT att.attisdropped) acls CROSS JOIN LATERAL pg_catalog.aclexplode(acls.acl) a WHERE a.grantee <> g.relowner AND a.privilege_type <> 'SELECT'))",
    " THEN RAISE EXCEPTION 'graphql-orm change journal registry is missing, not owned by the recorder''s owner, or writable by another role'; END IF;",
    " IF TG_ARGV[0] LIKE 'pg\\_temp%' OR TG_ARGV[0] LIKE 'pg\\_toast\\_temp%' OR TG_TABLE_SCHEMA::pg_catalog.text IS DISTINCT FROM TG_ARGV[1] OR TG_TABLE_NAME::pg_catalog.text IS DISTINCT FROM TG_ARGV[2]",
    " OR NOT EXISTS (SELECT 1 FROM pg_catalog.pg_class c WHERE c.oid = TG_RELID AND c.relpersistence <> 't' AND pg_catalog.pg_get_userbyid(c.relowner) = CURRENT_USER)",
    " OR NOT EXISTS (SELECT 1 FROM pg_catalog.pg_index i JOIN pg_catalog.pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = i.indkey[0] WHERE i.indrelid = TG_RELID AND i.indisprimary AND i.indnkeyatts = 1 AND a.attname::pg_catalog.text = TG_ARGV[5])",
    " OR NOT EXISTS (SELECT 1 FROM graphql_orm.change_journal_tables r WHERE r.journal_schema = TG_ARGV[0] AND r.declared_table = TG_ARGV[4] AND r.entity_name = TG_ARGV[3] AND r.key_column = TG_ARGV[5] AND r.relid::pg_catalog.oid = TG_RELID)",
    " THEN RAISE EXCEPTION 'graphql-orm change journal trigger does not match the table it fires on'; END IF;",
    " journal := pg_catalog.to_regclass(pg_catalog.format('%I.%I', TG_ARGV[0], '__graphql_orm_change_log'));",
    " IF journal IS NULL OR NOT EXISTS (SELECT 1 FROM pg_catalog.pg_class j WHERE j.oid = journal AND j.relkind = 'r' AND pg_catalog.pg_get_userbyid(j.relowner) = CURRENT_USER) THEN RAISE EXCEPTION 'graphql-orm change journal table is missing or not owned by its recorder'; END IF;",
    " EXECUTE pg_catalog.format('LOCK TABLE %I.%I IN ROW EXCLUSIVE MODE', TG_ARGV[0], '__graphql_orm_change_log');",
    " IF EXISTS (SELECT 1 FROM pg_catalog.pg_trigger t WHERE t.tgrelid = journal AND NOT t.tgisinternal) OR EXISTS (SELECT 1 FROM pg_catalog.pg_rewrite r WHERE r.ev_class = journal) THEN RAISE EXCEPTION 'graphql-orm change journal table carries a trigger or rule; remove it before journaling'; END IF;",
    " IF EXISTS (SELECT 1 FROM pg_catalog.pg_class j CROSS JOIN LATERAL pg_catalog.aclexplode(j.relacl) a WHERE j.oid = journal AND a.grantee <> j.relowner AND a.privilege_type = 'TRIGGER') THEN RAISE EXCEPTION 'graphql-orm change journal table grants TRIGGER to a role other than its owner; revoke it before journaling'; END IF;",
    " IF TG_ARGV[6] = 'bin' THEN key_sql := 'SELECT ''bin:''::pg_catalog.text || pg_catalog.encode(($1).%I, ''hex'')'; ELSIF TG_ARGV[6] = 'text' THEN key_sql := 'SELECT ($1).%I::pg_catalog.text'; ELSE RAISE EXCEPTION 'graphql-orm change journal trigger has an unknown key encoding'; END IF;",
    " IF TG_OP <> 'DELETE' THEN EXECUTE pg_catalog.format(key_sql, TG_ARGV[5]) INTO entry_key USING NEW; END IF;",
    " IF TG_OP <> 'INSERT' THEN EXECUTE pg_catalog.format(key_sql, TG_ARGV[5]) INTO previous_key USING OLD; END IF;",
    " changed_at := pg_catalog.floor(pg_catalog.date_part('epoch', pg_catalog.clock_timestamp()))::pg_catalog.int8;",
    " entry_sql := pg_catalog.format('INSERT INTO %I.%I (id, entity_name, table_name, primary_key, action, changed_at) VALUES (pg_catalog.gen_random_uuid(), $1, $2, $3, $4, $5)', TG_ARGV[0], '__graphql_orm_change_log');",
    " IF TG_OP = 'UPDATE' AND previous_key IS NOT DISTINCT FROM entry_key THEN EXECUTE entry_sql USING TG_ARGV[3], TG_ARGV[4], entry_key, 'update'::pg_catalog.text, changed_at;",
    " ELSE IF TG_OP <> 'INSERT' THEN EXECUTE entry_sql USING TG_ARGV[3], TG_ARGV[4], previous_key, 'delete'::pg_catalog.text, changed_at; END IF;",
    " IF TG_OP <> 'DELETE' THEN EXECUTE entry_sql USING TG_ARGV[3], TG_ARGV[4], entry_key, 'create'::pg_catalog.text, changed_at; END IF; END IF;",
    " RETURN NULL; END ",
);

/// One table whose changes the PostgreSQL change journal records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChangeJournalTable {
    pub(crate) entity_name: String,
    /// Table path, as declared on the entity.
    pub(crate) table_name: String,
    /// Whether the declared path has a single part, with no schema.
    pub(crate) bare: bool,
    /// Single primary-key column.
    pub(crate) primary_key: String,
    /// `bin` for binary keys, otherwise `text`; matches the ORM event id.
    pub(crate) encoding: &'static str,
}

/// Unambiguous key for a live table: its quoted schema and name.
pub(crate) fn change_journal_table_key(schema: &str, name: &str) -> String {
    let quote = |identifier: &str| format!("\"{}\"", identifier.replace('"', "\"\""));
    format!("{}.{}", quote(schema), quote(name))
}

/// The declared path rendered exactly as the ORM's own SQL renders it, so
/// PostgreSQL resolves it to the table the ORM writes: plain identifiers stay
/// unquoted and fold, others are quoted.
pub(crate) fn change_journal_orm_path(declared: &str) -> String {
    DatabaseBackend::Postgres
        .render_identifier_path(declared)
        .into_owned()
}

impl ChangeJournalTable {
    /// Trigger arguments after the journal schema and the resolved table
    /// schema and name, in order.
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
                bare: super::dialect::identifier_path_is_bare(&table.table_name),
                primary_key: primary_key.clone(),
                encoding: change_journal_key_encoding(sql_type),
            })
        })
        .collect::<Vec<_>>();
    tables.sort_by(|left, right| left.table_name.cmp(&right.table_name));
    tables.dedup_by(|left, right| left.table_name == right.table_name);
    tables
}

/// PL/pgSQL statements revoking every table and column privilege other than
/// `SELECT` that a role other than its owner holds on the relation in the
/// `target` variable, `PUBLIC` included, and raising if one survives. They
/// use the variables `grantee_name`, `privilege_name` and `column_name`.
fn revoke_beyond_select_statements(what: &str) -> String {
    format!(
        "FOR grantee_name, privilege_name IN SELECT DISTINCT CASE WHEN a.grantee = 0 THEN 'PUBLIC' ELSE pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(a.grantee)) END, a.privilege_type FROM pg_catalog.pg_class c CROSS JOIN LATERAL pg_catalog.aclexplode(c.relacl) a WHERE c.oid = target AND a.grantee <> c.relowner AND a.privilege_type <> 'SELECT' LOOP \
         EXECUTE pg_catalog.format('REVOKE %s ON %s FROM %s CASCADE', privilege_name, target, grantee_name); END LOOP; \
         FOR grantee_name, privilege_name, column_name IN SELECT DISTINCT CASE WHEN a.grantee = 0 THEN 'PUBLIC' ELSE pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(a.grantee)) END, a.privilege_type, pg_catalog.quote_ident(att.attname) FROM pg_catalog.pg_class c JOIN pg_catalog.pg_attribute att ON att.attrelid = c.oid AND att.attnum > 0 AND NOT att.attisdropped CROSS JOIN LATERAL pg_catalog.aclexplode(att.attacl) a WHERE c.oid = target AND a.grantee <> c.relowner AND a.privilege_type <> 'SELECT' LOOP \
         EXECUTE pg_catalog.format('REVOKE %s (%s) ON %s FROM %s CASCADE', privilege_name, column_name, target, grantee_name); END LOOP; \
         IF EXISTS (SELECT 1 FROM pg_catalog.pg_class c CROSS JOIN LATERAL pg_catalog.aclexplode(c.relacl) a WHERE c.oid = target AND a.grantee <> c.relowner AND a.privilege_type <> 'SELECT') \
            OR EXISTS (SELECT 1 FROM pg_catalog.pg_class c JOIN pg_catalog.pg_attribute att ON att.attrelid = c.oid AND att.attnum > 0 AND NOT att.attisdropped CROSS JOIN LATERAL pg_catalog.aclexplode(att.attacl) a WHERE c.oid = target AND a.grantee <> c.relowner AND a.privilege_type <> 'SELECT') \
         THEN RAISE EXCEPTION USING ERRCODE = '42501', MESSAGE = 'graphql-orm could not revoke every non-owner table or column privilege beyond SELECT on the {what}; revoke the remaining grants as their grantor and re-apply'; END IF;"
    )
}

/// Run [`revoke_beyond_select_statements`] for `regclass_literal`, when the
/// relation exists, preceded by `before`.
fn render_secure_relation(regclass_literal: &str, before: &str, what: &str) -> String {
    format!(
        "DO $graphql_orm$ DECLARE target pg_catalog.regclass := pg_catalog.to_regclass({regclass_literal}); grantee_name pg_catalog.text; privilege_name pg_catalog.text; column_name pg_catalog.text; BEGIN IF target IS NOT NULL THEN {before} {} END IF; END $graphql_orm$",
        revoke_beyond_select_statements(what)
    )
}

/// Create the PostgreSQL change journal and its database-side recorder.
///
/// The statements create and secure `__graphql_orm_change_log` (see
/// [`postgres_change_log_rls_sql`]) and the registry
/// [`CHANGE_JOURNAL_REGISTRY`], which only the owner can write and every role
/// can read (it holds what `pg_trigger.tgargs` already shows). They create
/// `graphql_orm.record_change_journal_entry()` as `SECURITY DEFINER` with
/// `search_path = pg_catalog, pg_temp`, revoke its `EXECUTE` from every role
/// other than its owner, and, for each table, record its binding in the
/// registry and (re)create an `AFTER INSERT OR UPDATE OR DELETE` row trigger
/// named `graphql_orm_change_journal`. Each binding resolves the declared path
/// exactly as the ORM's writes render it.
///
/// They also grant `USAGE` on schema `graphql_orm` to `PUBLIC`, so every
/// journaling role can read the registry.
///
/// Run them as the role that owns the journal and the tables. The recorder
/// runs as that owner, so the runtime role needs no privilege on the journal.
pub(crate) fn postgres_change_journal_recorder_sql(tables: &[ChangeJournalTable]) -> Vec<String> {
    let registry_literal = sql_string_literal(CHANGE_JOURNAL_REGISTRY);
    let mut statements = vec![
        "CREATE SCHEMA IF NOT EXISTS graphql_orm".to_string(),
        // Every journaling role reads the registry and calls the helpers here.
        // Nothing in the schema is writable or callable through this grant:
        // the recorder's EXECUTE and the registry's and journal's ACLs are
        // enforced separately.
        "GRANT USAGE ON SCHEMA graphql_orm TO PUBLIC".to_string(),
        postgres_change_journal_table_sql(),
        postgres_change_log_rls_sql(),
        format!(
            "CREATE TABLE IF NOT EXISTS {CHANGE_JOURNAL_REGISTRY} (journal_schema pg_catalog.text NOT NULL, declared_table pg_catalog.text NOT NULL, entity_name pg_catalog.text NOT NULL, key_column pg_catalog.text NOT NULL, relid pg_catalog.regclass NOT NULL, PRIMARY KEY (journal_schema, declared_table))"
        ),
        render_secure_relation(&registry_literal, "", "change journal registry"),
        format!("GRANT SELECT ON {CHANGE_JOURNAL_REGISTRY} TO PUBLIC"),
        // Bindings whose table no longer exists bind nothing.
        format!(
            "DELETE FROM {CHANGE_JOURNAL_REGISTRY} r WHERE NOT EXISTS (SELECT 1 FROM pg_catalog.pg_class c WHERE c.oid = r.relid::pg_catalog.oid)"
        ),
        format!(
            "CREATE OR REPLACE FUNCTION graphql_orm.{CHANGE_JOURNAL_FUNCTION}() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, pg_temp AS $graphql_orm${CHANGE_JOURNAL_FUNCTION_BODY}$graphql_orm$"
        ),
        format!("REVOKE ALL ON FUNCTION graphql_orm.{CHANGE_JOURNAL_FUNCTION}() FROM PUBLIC"),
        format!(
            "DO $graphql_orm$ DECLARE recorder pg_catalog.regprocedure := pg_catalog.to_regprocedure('graphql_orm.{CHANGE_JOURNAL_FUNCTION}()'); grantee_name pg_catalog.text; BEGIN \
             FOR grantee_name IN SELECT DISTINCT CASE WHEN a.grantee = 0 THEN 'PUBLIC' ELSE pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(a.grantee)) END FROM pg_catalog.pg_proc p CROSS JOIN LATERAL pg_catalog.aclexplode(p.proacl) a WHERE p.oid = recorder AND a.grantee <> p.proowner LOOP \
             EXECUTE pg_catalog.format('REVOKE ALL ON FUNCTION %s FROM %s CASCADE', recorder, grantee_name); END LOOP; END $graphql_orm$"
        ),
        // graphql-orm only installs the recorder as `graphql_orm_change_journal`;
        // a trigger calling it under any other name is never the plan's. Drop
        // those on tables the applying role has the owner's privileges on;
        // validation reports the rest. `tgparentid` (PostgreSQL 13+) skips
        // partition clones, which go with their parent.
        format!(
            "DO $graphql_orm$ DECLARE stray pg_catalog.record; BEGIN \
             FOR stray IN SELECT t.tgname, t.tgrelid::pg_catalog.regclass AS target FROM pg_catalog.pg_trigger t JOIN pg_catalog.pg_class c ON c.oid = t.tgrelid WHERE t.tgfoid = pg_catalog.to_regprocedure('graphql_orm.{CHANGE_JOURNAL_FUNCTION}()') AND t.tgname <> {trigger} AND t.tgparentid = 0 AND pg_catalog.pg_has_role(c.relowner, 'USAGE') LOOP \
             EXECUTE pg_catalog.format('DROP TRIGGER %I ON %s', stray.tgname, stray.target); END LOOP; END $graphql_orm$",
            trigger = sql_string_literal(CHANGE_JOURNAL_TRIGGER),
        ),
    ];
    statements.extend(tables.iter().map(render_change_journal_trigger_sql));
    statements
}

/// Bind one table in the registry and (re)create its change journal trigger.
///
/// The declared path is resolved once, here, exactly as the ORM's writes
/// render it; the journal too. Both are recorded by schema and name in the
/// trigger arguments, and the table's identity in the registry.
fn render_change_journal_trigger_sql(table: &ChangeJournalTable) -> String {
    let backend = DatabaseBackend::Postgres;
    let base = change_journal_orm_path(&table.table_name);
    let journal_regclass = sql_string_literal(&backend.quote_identifier(CHANGE_LOG_TABLE));
    let [entity, table_name, key, encoding] = table.trigger_arguments().map(sql_string_literal);
    // A bare name must resolve in the journal's schema, where the ORM's
    // unqualified names and the journal itself resolve.
    let binding = if table.bare {
        "target_ns = journal_ns"
    } else {
        "true"
    };
    format!(
        "DO $graphql_orm$ DECLARE journal pg_catalog.regclass := pg_catalog.to_regclass({journal_regclass}); target pg_catalog.regclass := pg_catalog.to_regclass({base_literal}); journal_ns pg_catalog.text; target_ns pg_catalog.text; target_rel pg_catalog.text; BEGIN \
         IF journal IS NULL THEN RAISE EXCEPTION USING ERRCODE = '42P01', MESSAGE = 'graphql-orm change journal table does not exist'; END IF; \
         IF target IS NULL THEN RAISE EXCEPTION USING ERRCODE = '42P01', MESSAGE = format('graphql-orm table %s does not exist', {base_literal}); END IF; \
         SELECT n.nspname INTO journal_ns FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE c.oid = journal; \
         SELECT n.nspname, c.relname INTO target_ns, target_rel FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE c.oid = target; \
         IF journal_ns LIKE 'pg\\_temp%' OR journal_ns LIKE 'pg\\_toast\\_temp%' OR target_ns LIKE 'pg\\_temp%' OR target_ns LIKE 'pg\\_toast\\_temp%' THEN RAISE EXCEPTION USING ERRCODE = '42P01', MESSAGE = 'graphql-orm change journal trigger resolves to a temporary table'; END IF; \
         IF NOT ({binding}) THEN RAISE EXCEPTION USING ERRCODE = '42P01', MESSAGE = format('graphql-orm change journal cannot bind table %s: it resolves to %I.%I, and a bare table name must resolve in the journal schema %I; qualify the entity table', {base_literal}, target_ns, target_rel, journal_ns); END IF; \
         INSERT INTO {CHANGE_JOURNAL_REGISTRY} (journal_schema, declared_table, entity_name, key_column, relid) VALUES (journal_ns, {table_name}, {entity}, {key}, target) \
         ON CONFLICT (journal_schema, declared_table) DO UPDATE SET entity_name = EXCLUDED.entity_name, key_column = EXCLUDED.key_column, relid = EXCLUDED.relid; \
         EXECUTE format('DROP TRIGGER IF EXISTS %I ON %s', {trigger_name}, target); \
         EXECUTE format('CREATE TRIGGER %I AFTER INSERT OR UPDATE OR DELETE ON %s FOR EACH ROW EXECUTE FUNCTION graphql_orm.{CHANGE_JOURNAL_FUNCTION}(%L, %L, %L, %L, %L, %L, %L)', {trigger_name}, target, journal_ns, target_ns, target_rel, {entity}, {table_name}, {key}, {encoding}); \
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
/// configured predicate. The generated policy of every other operation is
/// dropped, so removing an operation from `#[graphql_rls]` removes its policy.
pub fn render_postgres_entity_rls_sql(entity: &RlsEntityMetadata) -> Vec<String> {
    let table = DatabaseBackend::Postgres.render_identifier_path(entity.table_name);
    let mut statements = vec![format!("ALTER TABLE {table} ENABLE ROW LEVEL SECURITY")];
    if entity.force {
        statements.push(format!("ALTER TABLE {table} FORCE ROW LEVEL SECURITY"));
    }

    for operation in RLS_OPERATIONS {
        let policy_name =
            DatabaseBackend::Postgres.quote_identifier(&policy_name(entity.table_name, operation));
        statements.push(format!("DROP POLICY IF EXISTS {policy_name} ON {table}"));
        for policy in sorted_policies(entity) {
            if policy.operation != operation {
                continue;
            }
            let Some(predicate) = render_policy_predicate(DatabaseBackend::Postgres, policy) else {
                continue;
            };
            statements.push(render_create_policy_statement(
                &policy_name,
                &table,
                operation,
                &predicate,
            ));
        }
    }

    statements
}

/// Every policy operation, in policy-name order.
const RLS_OPERATIONS: [RlsOperation; 4] = [
    RlsOperation::Select,
    RlsOperation::Insert,
    RlsOperation::Update,
    RlsOperation::Delete,
];

/// Render the SQL predicate for one operation policy.
///
/// Custom predicates are returned exactly. Generated predicates combine scope,
/// tenant, and owner conditions in deterministic order, each in parentheses.
/// Tenant and owner columns are always quoted as one identifier, so a value
/// that is not a plain column name cannot change the predicate. Returns `None`
/// when the operation intentionally has no policy.
pub fn render_policy_predicate(
    _backend: DatabaseBackend,
    policy: &RlsOperationPolicy,
) -> Option<String> {
    if let Some(predicate) = policy.predicate {
        return Some(predicate.to_string());
    }

    let mut conditions = Vec::new();
    if let Some(scope) = policy.scope {
        conditions.push(format!(
            "graphql_orm.has_scope({})",
            sql_string_literal(scope)
        ));
    }
    if let Some(column) = policy.tenant_column {
        conditions.push(format!(
            "{} = graphql_orm.current_tenant_id()",
            quote_policy_column(column)
        ));
    }
    if let Some(column) = policy.owner_column {
        conditions.push(format!(
            "{} = graphql_orm.current_user_id()",
            quote_policy_column(column)
        ));
    }

    if conditions.is_empty() {
        None
    } else {
        Some(
            conditions
                .iter()
                .map(|condition| format!("({condition})"))
                .collect::<Vec<_>>()
                .join(" AND "),
        )
    }
}

/// Render a policy column as exactly one PostgreSQL identifier naming the
/// column the ORM's DDL creates. As the DDL does, a plain name stays bare and
/// exactly one well-formed quoted identifier is kept; anything else is quoted
/// in full, so the text cannot change the predicate.
fn quote_policy_column(column: &str) -> String {
    if super::dialect::postgres_delimited_identifier(column).is_some() {
        return column.to_string();
    }
    let rendered = DatabaseBackend::Postgres.render_identifier(column);
    if rendered == column && !column.starts_with('"') {
        rendered.into_owned()
    } else {
        format!("\"{}\"", column.replace('"', "\"\""))
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

/// One table whose RLS state target validation reads, with the declared
/// policy expressions PostgreSQL renders for comparison.
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RlsTableProbe {
    /// Table path exactly as the plan's statements render it.
    pub table_sql: String,
    /// Declared policy expressions on the table, without duplicates.
    pub expressions: Vec<RlsProbeExpression>,
    /// Whether to render the declared and live policy expressions. Without
    /// it only the table and its policies' other attributes are read.
    pub render: bool,
}

/// One declared policy expression of an [`RlsTableProbe`].
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RlsProbeExpression {
    /// The predicate exactly as the plan writes it; the key of
    /// [`LiveRlsTableState::expected`].
    pub declared: String,
    /// For a search-document predicate, its parts. Its base table is resolved
    /// like the probe's table and the predicate rendered with the resolved,
    /// schema-qualified base, so it compares under a pinned `search_path`.
    pub search_document: Option<RlsSearchDocumentProbe>,
}

/// The parts of a search-document predicate, see [`RlsProbeExpression`].
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RlsSearchDocumentProbe {
    /// Base table path as the plan's SQL writes it.
    pub base_table_sql: String,
    /// Search table as the plan's SQL writes it; also the column qualifier.
    pub search_table_sql: String,
    /// Base primary-key column.
    pub primary_key: String,
    /// Declared base primary-key SQL type.
    pub primary_key_sql_type: String,
}

fn search_document_probe(search: &RlsSearchTable) -> RlsSearchDocumentProbe {
    RlsSearchDocumentProbe {
        base_table_sql: DatabaseBackend::Postgres
            .render_identifier_path(&search.base_table)
            .into_owned(),
        search_table_sql: search_table_sql(search),
        primary_key: search.primary_key.clone(),
        primary_key_sql_type: search.primary_key_sql_type.clone(),
    }
}

/// A policy command as `pg_policy.polcmd` records it.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveRlsCommand {
    /// `FOR SELECT`.
    Select,
    /// `FOR INSERT`.
    Insert,
    /// `FOR UPDATE`.
    Update,
    /// `FOR DELETE`.
    Delete,
    /// `FOR ALL`.
    All,
}

impl LiveRlsCommand {
    /// The SQL keyword after `FOR`.
    pub const fn sql(self) -> &'static str {
        match self {
            Self::Select => "SELECT",
            Self::Insert => "INSERT",
            Self::Update => "UPDATE",
            Self::Delete => "DELETE",
            Self::All => "ALL",
        }
    }

    fn of(operation: RlsOperation) -> Self {
        match operation {
            RlsOperation::Select => Self::Select,
            RlsOperation::Insert => Self::Insert,
            RlsOperation::Update => Self::Update,
            RlsOperation::Delete => Self::Delete,
        }
    }
}

/// PostgreSQL's rendering of one policy expression, or why it could not
/// render it.
#[doc(hidden)]
pub type RenderedRlsExpression = Result<String, String>;

/// One live policy on a probed table.
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveRlsPolicyState {
    /// `pg_policy.polname`.
    pub name: String,
    /// `pg_policy.polcmd`.
    pub command: LiveRlsCommand,
    /// `pg_policy.polpermissive`.
    pub permissive: bool,
    /// Role names the policy applies to; `public` stands for `PUBLIC`.
    pub roles: Vec<String>,
    /// Rendered `USING` expression, when the policy has one.
    pub using: Option<RenderedRlsExpression>,
    /// Rendered `WITH CHECK` expression, when the policy has one.
    pub check: Option<RenderedRlsExpression>,
}

/// Live RLS state of one probed table.
#[doc(hidden)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveRlsTableState {
    /// [`RlsTableProbe::table_sql`] of the probe.
    pub table_sql: String,
    /// Schema and name the path resolves to; `None` when it does not resolve.
    pub relation: Option<(String, String)>,
    /// `pg_class.relrowsecurity`.
    pub enabled: bool,
    /// `pg_class.relforcerowsecurity`.
    pub forced: bool,
    /// `row_security_active(table)` for the introspecting role.
    pub row_security_active: bool,
    /// Every policy on the table, `FOR ALL` and restrictive ones included.
    pub policies: Vec<LiveRlsPolicyState>,
    /// Each probe expression with PostgreSQL's rendering of it.
    pub expected: Vec<(String, RenderedRlsExpression)>,
}

/// One live function in schema `graphql_orm` named like an RLS helper.
#[doc(hidden)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveRlsHelperFunction {
    /// `pg_proc.proname`.
    pub name: String,
    /// `pg_get_function_identity_arguments`.
    pub arguments: String,
    /// `pg_get_function_result`.
    pub returns: String,
    /// `pg_proc.prosrc`.
    pub body: String,
    /// `pg_language.lanname`.
    pub language: String,
    /// `pg_proc.provolatile`.
    pub volatility: String,
    /// `pg_proc.prokind`.
    pub kind: String,
    /// `pg_proc.prosecdef`.
    pub security_definer: bool,
    /// `pg_proc.proisstrict`.
    pub strict: bool,
    /// `pg_proc.proretset`.
    pub returns_set: bool,
    /// `pg_proc.proconfig`.
    pub config: Vec<String>,
}

/// Live RLS state read for target validation.
#[doc(hidden)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveRlsTarget {
    /// One state per probe, in probe order.
    pub tables: Vec<LiveRlsTableState>,
    /// Functions in `graphql_orm` named like an RLS helper.
    pub helper_functions: Vec<LiveRlsHelperFunction>,
}

/// One policy the target declares on a table.
struct DeclaredPolicy {
    name: String,
    operation: RlsOperation,
    predicate: String,
}

/// Tables target validation probes: every RLS entity table, then every
/// search document table of an RLS entity, each with its declared policy
/// expressions.
pub(crate) fn rls_table_probes(target: &SchemaTarget) -> Vec<RlsTableProbe> {
    let mut probes: Vec<RlsTableProbe> = Vec::new();
    let mut add = |table_sql: String, expressions: Vec<RlsProbeExpression>| {
        let index = match probes.iter().position(|probe| probe.table_sql == table_sql) {
            Some(index) => index,
            None => {
                probes.push(RlsTableProbe {
                    table_sql,
                    expressions: Vec::new(),
                    render: true,
                });
                probes.len() - 1
            }
        };
        for expression in expressions {
            if !probes[index]
                .expressions
                .iter()
                .any(|known| known.declared == expression.declared)
            {
                probes[index].expressions.push(expression);
            }
        }
    };
    for entity in sorted_entities(&target.rls) {
        add(
            entity_table_sql(entity),
            declared_entity_policies(entity)
                .into_iter()
                .map(|policy| RlsProbeExpression {
                    declared: policy.predicate,
                    search_document: None,
                })
                .collect(),
        );
    }
    for search in rls_search_tables(target) {
        add(
            search_table_sql(&search),
            vec![RlsProbeExpression {
                declared: render_search_document_predicate(&search),
                search_document: Some(search_document_probe(&search)),
            }],
        );
    }
    probes
}

fn entity_table_sql(entity: &RlsEntityMetadata) -> String {
    DatabaseBackend::Postgres
        .render_identifier_path(entity.table_name)
        .into_owned()
}

/// The search table as the ORM's search SQL names it.
fn search_table_sql(search: &RlsSearchTable) -> String {
    DatabaseBackend::Postgres
        .render_identifier(&search.search_table)
        .into_owned()
}

/// Flattened [`LiveRlsEnforcement`] key of the table a declared path resolves
/// to when the ORM's SQL renders it.
fn resolved_table_key(table_name: &str) -> String {
    let joined = super::dialect::postgres_resolved_identifier_parts(table_name).join("_");
    joined
        .strip_prefix("public_")
        .map(str::to_string)
        .unwrap_or(joined)
}

fn declared_entity_policies(entity: &RlsEntityMetadata) -> Vec<DeclaredPolicy> {
    sorted_policies(entity)
        .into_iter()
        .filter_map(|policy| {
            render_policy_predicate(DatabaseBackend::Postgres, policy).map(|predicate| {
                DeclaredPolicy {
                    name: policy_name(entity.table_name, policy.operation),
                    operation: policy.operation,
                    predicate,
                }
            })
        })
        .collect()
}

/// Validate RLS entity tables and the RLS helper functions against the live
/// state read for `target`.
///
/// Each declared policy must exist under its generated name as a
/// `PERMISSIVE` policy for `PUBLIC` with the declared command, and
/// PostgreSQL's rendering of its live expressions must equal its rendering of
/// the declared predicate, both parsed against the same table with
/// `search_path` pinned to `pg_catalog, pg_temp`.
/// Any other policy on the table is an error. An expression PostgreSQL could
/// not render is reported as an error, never treated as matching.
pub(crate) fn validate_rls_models(
    backend: DatabaseBackend,
    policy: SchemaPolicy,
    live: &LiveRlsTarget,
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

    let mut diagnostics = Vec::new();
    for entity in sorted_entities(target) {
        let table_name = entity.table_name;
        let Some(state) = live_table_state(live, &entity_table_sql(entity)) else {
            diagnostics.push(rls_diagnostic(
                SchemaDiagnosticSeverity::Error,
                table_name,
                format!("Missing RLS table state for {table_name}"),
            ));
            continue;
        };
        if !state.enabled {
            diagnostics.push(rls_diagnostic(
                SchemaDiagnosticSeverity::Error,
                table_name,
                format!("RLS is not enabled on {table_name}"),
            ));
        }
        if entity.force && !state.forced {
            diagnostics.push(rls_diagnostic(
                SchemaDiagnosticSeverity::Error,
                table_name,
                format!("RLS is not forced on {table_name}"),
            ));
        }
        validate_declared_policies(
            &mut diagnostics,
            table_name,
            state,
            &declared_entity_policies(entity),
            "the declared predicate",
        );
    }
    validate_rls_helper_functions(&mut diagnostics, live);
    diagnostics
}

/// The probed state of `table_sql`, when its table resolved.
fn live_table_state<'a>(live: &'a LiveRlsTarget, table_sql: &str) -> Option<&'a LiveRlsTableState> {
    live.tables
        .iter()
        .find(|state| state.table_sql == table_sql && state.relation.is_some())
}

/// Compare a table's live policies with exactly the `declared` set.
fn validate_declared_policies(
    diagnostics: &mut Vec<SchemaDiagnostic>,
    table_name: &str,
    state: &LiveRlsTableState,
    declared: &[DeclaredPolicy],
    predicate_label: &str,
) {
    let mut error = |message: String| {
        diagnostics.push(rls_diagnostic(
            SchemaDiagnosticSeverity::Error,
            table_name,
            message,
        ));
    };
    for expected in declared {
        let name = &expected.name;
        let Some(live) = state.policies.iter().find(|live| &live.name == name) else {
            error(format!("Missing RLS policy {name} on {table_name}"));
            continue;
        };
        let command = LiveRlsCommand::of(expected.operation);
        if live.command != command {
            error(format!(
                "RLS policy {name} on {table_name} is FOR {}, not FOR {}",
                live.command.sql(),
                command.sql()
            ));
            continue;
        }
        if !live.permissive {
            error(format!(
                "RLS policy {name} on {table_name} is RESTRICTIVE, not PERMISSIVE"
            ));
        }
        if live.roles != ["public"] {
            error(format!(
                "RLS policy {name} on {table_name} applies to roles {}, not PUBLIC",
                live.roles.join(", ")
            ));
        }
        let rendered = state
            .expected
            .iter()
            .find(|(expression, _)| expression == &expected.predicate)
            .map(|(_, rendered)| rendered);
        let rendered = match rendered {
            Some(Ok(rendered)) => rendered,
            Some(Err(reason)) => {
                error(format!(
                    "RLS policy {name} on {table_name} could not be compared: PostgreSQL did not render {predicate_label}: {reason}"
                ));
                continue;
            }
            None => {
                error(format!(
                    "RLS policy {name} on {table_name} could not be compared: {predicate_label} was not rendered"
                ));
                continue;
            }
        };
        let clauses: &[(&str, &Option<RenderedRlsExpression>)] = match expected.operation {
            RlsOperation::Select | RlsOperation::Delete => &[("USING", &live.using)],
            RlsOperation::Insert => &[("WITH CHECK", &live.check)],
            RlsOperation::Update => &[("USING", &live.using), ("WITH CHECK", &live.check)],
        };
        for (clause, live_expression) in clauses {
            match live_expression {
                Some(Ok(live_expression)) if live_expression == rendered => {}
                Some(Ok(_)) => error(format!(
                    "RLS policy {name} on {table_name} {clause} expression differs from {predicate_label}"
                )),
                Some(Err(reason)) => error(format!(
                    "RLS policy {name} on {table_name} could not be compared: PostgreSQL did not render its live {clause} expression: {reason}"
                )),
                None => error(format!(
                    "RLS policy {name} on {table_name} has no {clause} expression; it differs from {predicate_label}"
                )),
            }
        }
    }
    let generated = RLS_OPERATIONS.map(|operation| policy_name(table_name, operation));
    for live in &state.policies {
        if declared.iter().any(|expected| expected.name == live.name) {
            continue;
        }
        let kind = if live.permissive {
            "PERMISSIVE"
        } else {
            "RESTRICTIVE"
        };
        let reason = if generated.contains(&live.name) {
            "is a graphql-orm policy for an operation the target has not declared; re-apply the schema target to drop it"
        } else {
            "is not declared by the target, and PostgreSQL combines it with the declared policies; drop it"
        };
        error(format!(
            "Unexpected RLS policy {} on {table_name} (FOR {}, {kind}) {reason}",
            live.name,
            live.command.sql()
        ));
    }
}

/// Report RLS helper functions that are missing or differ from the
/// definitions the plan installs.
fn validate_rls_helper_functions(diagnostics: &mut Vec<SchemaDiagnostic>, live: &LiveRlsTarget) {
    for helper in RLS_HELPER_FUNCTIONS {
        let signature = format!("graphql_orm.{}({})", helper.name, helper.arguments);
        let found = live.helper_functions.iter().find(|function| {
            function.name == helper.name && function.arguments == helper.arguments
        });
        let message = match found {
            None => format!("Missing RLS helper function {signature}; re-apply the schema target"),
            Some(function)
                if function.body == helper.body
                    && function.returns == helper.returns
                    && function.language == "sql"
                    && function.volatility == "s"
                    && function.kind == "f"
                    && !function.security_definer
                    && !function.strict
                    && !function.returns_set
                    && function.config.is_empty() =>
            {
                continue;
            }
            Some(_) => format!(
                "RLS helper function {signature} differs from the definition graphql-orm installs; re-apply the schema target"
            ),
        };
        diagnostics.push(SchemaDiagnostic {
            severity: SchemaDiagnosticSeverity::Error,
            kind: SchemaDiagnosticKind::RlsMismatch,
            table: None,
            column: None,
            message,
        });
    }
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
    /// ORM change journal triggers, by [`change_journal_table_key`].
    pub change_journal_triggers: std::collections::BTreeMap<String, LiveChangeJournalTrigger>,
    /// Schema of the change journal the connection's `search_path` resolves.
    pub change_journal_schema: Option<String>,
    /// Rows of the change journal registry.
    pub change_journal_registry: Vec<LiveChangeJournalBinding>,
    /// Whether the change journal registry exists.
    pub change_journal_registry_present: bool,
    /// Whether the registry's owner also owns the recorder function.
    pub change_journal_registry_owned_by_recorder_owner: bool,
    /// Roles other than the registry owner (`PUBLIC` included) holding a
    /// table or column privilege other than `SELECT` on the registry.
    pub change_journal_registry_writers: Vec<String>,
    /// Live `graphql_orm.record_change_journal_entry()` definition, when present.
    pub change_journal_function: Option<LiveChangeJournalFunction>,
    /// Every trigger that calls `graphql_orm.record_change_journal_entry()`,
    /// as `(`[`change_journal_table_key`]`, trigger name)`.
    pub change_journal_recorder_triggers: Vec<(String, String)>,
    /// Whether the current role is a member of the change journal's owner
    /// (a superuser is a member of every role).
    pub change_journal_owned_by_current_role: bool,
    /// Non-internal triggers and rules on the change journal, by name.
    pub change_journal_table_triggers: Vec<String>,
    /// Roles other than the journal owner (`PUBLIC` included) holding a
    /// table or column privilege other than `SELECT` on the change journal.
    pub change_journal_writers: Vec<String>,
    /// Roles other than the recorder owner (`PUBLIC` included) holding
    /// `EXECUTE` on `graphql_orm.record_change_journal_entry()`.
    pub change_journal_recorder_executors: Vec<String>,
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
    /// Schema of the table the trigger is on.
    pub relation_schema: String,
    /// Name of the table the trigger is on.
    pub relation_name: String,
    /// Whether the schema named by the first argument holds a change journal
    /// owned by the function owner.
    pub journal_owned_by_function_owner: bool,
}

/// One row of the change journal registry.
#[doc(hidden)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveChangeJournalBinding {
    /// Journal schema the binding belongs to.
    pub journal_schema: String,
    /// Declared table path.
    pub declared_table: String,
    /// Bound entity name.
    pub entity_name: String,
    /// Bound key column.
    pub key_column: String,
    /// Schema and name of the bound table, when it still exists.
    pub relation: Option<(String, String)>,
    /// Schema and name the ORM's writes resolve the declared path to through
    /// the connection's `search_path`, when it resolves.
    pub orm_relation: Option<(String, String)>,
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
    probed: &LiveRlsTarget,
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

    // Entity and search tables are resolved by the path the ORM's SQL uses;
    // the change journal by its table key.
    let mut protected = sorted_entities(&target.rls)
        .into_iter()
        .map(|entity| {
            let active = live_table_state(probed, &entity_table_sql(entity))
                .map(|state| state.row_security_active);
            (entity.table_name.to_string(), active)
        })
        .collect::<Vec<_>>();
    protected.extend(rls_search_tables(target).into_iter().map(|search| {
        let active = live_table_state(probed, &search_table_sql(&search))
            .map(|state| state.row_security_active);
        (search.search_table, active)
    }));
    protected.push((
        CHANGE_LOG_TABLE.to_string(),
        live.row_security_active.get(CHANGE_LOG_TABLE).copied(),
    ));
    for (table, active) in protected {
        if active == Some(false) {
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

/// Validate RLS on the ORM-managed search document tables of RLS entities.
///
/// Each search table must carry exactly the four generated policies exposing
/// a document while its base row is visible, compared as in
/// [`validate_rls_models`], and its base table an intact delete trigger.
pub(crate) fn validate_protected_rls_tables(
    backend: DatabaseBackend,
    policy: SchemaPolicy,
    current: &LiveRlsTarget,
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
    let mut diagnostics = Vec::new();

    for search in rls_search_tables(target) {
        let table_name = search.search_table.as_str();
        let Some(current_table) = live_table_state(current, &search_table_sql(&search)) else {
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
        let predicate = render_search_document_predicate(&search);
        let declared = RLS_OPERATIONS.map(|operation| DeclaredPolicy {
            name: policy_name(table_name, operation),
            operation,
            predicate: predicate.clone(),
        });
        validate_declared_policies(
            &mut diagnostics,
            table_name,
            current_table,
            &declared,
            "the visible base row predicate",
        );
        if let Some(live) = live
            && let Some(message) = search_delete_trigger_problem(live, &search)
        {
            diagnostics.push(rls_diagnostic(
                SchemaDiagnosticSeverity::Error,
                table_name,
                message,
            ));
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
            if let Some(live) = live
                && live.change_journal_owned_by_current_role
            {
                diagnostics.push(error(format!(
                    "Current database role {} acts as the journal owner of {CHANGE_LOG_TABLE}: it bypasses the journal's row security and every recorder privilege check. Validate with the runtime pool, and do not let the runtime role own the journal",
                    live.role
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
    for name in &live.change_journal_table_triggers {
        diagnostics.push(error(format!(
            "Trigger or rule {name} on {CHANGE_LOG_TABLE} would run as the journal owner; remove it"
        )));
    }
    for role in &live.change_journal_writers {
        diagnostics.push(error(format!(
            "Role {role} holds a privilege other than SELECT on {CHANGE_LOG_TABLE}; revoke it"
        )));
    }
    for role in &live.change_journal_recorder_executors {
        diagnostics.push(error(format!(
            "Role {role} holds EXECUTE on graphql_orm.{CHANGE_JOURNAL_FUNCTION}() and could attach it to another trigger; revoke it"
        )));
    }
    if !live.change_journal_registry_present {
        diagnostics.push(error(format!(
            "Missing change journal registry {CHANGE_JOURNAL_REGISTRY}; re-apply the schema target"
        )));
    } else if !live.change_journal_registry_owned_by_recorder_owner {
        diagnostics.push(error(format!(
            "Change journal registry {CHANGE_JOURNAL_REGISTRY} is not owned by the owner of graphql_orm.{CHANGE_JOURNAL_FUNCTION}()"
        )));
    }
    for role in &live.change_journal_registry_writers {
        diagnostics.push(error(format!(
            "Role {role} holds a privilege other than SELECT on {CHANGE_JOURNAL_REGISTRY} and could bind tables to the recorder; revoke it"
        )));
    }
    let journal_schema = live.change_journal_schema.as_deref();
    let tables = change_journal_tables(&target.schema);
    let bindings = tables
        .iter()
        .map(|table| {
            let binding = live.change_journal_registry.iter().find(|binding| {
                Some(binding.journal_schema.as_str()) == journal_schema
                    && binding.declared_table == table.table_name
            });
            (table, binding)
        })
        .collect::<Vec<_>>();
    let managed = bindings
        .iter()
        .filter_map(|(_, binding)| binding.and_then(|binding| binding.relation.as_ref()))
        .map(|(schema, name)| change_journal_table_key(schema, name))
        .collect::<std::collections::BTreeSet<_>>();
    for (table, trigger) in &live.change_journal_recorder_triggers {
        if trigger != CHANGE_JOURNAL_TRIGGER {
            diagnostics.push(rls_diagnostic(
                SchemaDiagnosticSeverity::Error,
                table,
                format!(
                    "Trigger {trigger} on {table} calls graphql_orm.{CHANGE_JOURNAL_FUNCTION}() under a name other than {CHANGE_JOURNAL_TRIGGER}; drop it"
                ),
            ));
        } else if !managed.contains(table) {
            // The registry binds this trigger's declared table to the table it
            // fires on, so it can only record that table's own changes. Another
            // schema target in the same database may own it; nothing drops it.
            diagnostics.push(rls_diagnostic(
                SchemaDiagnosticSeverity::Warning,
                table,
                format!(
                    "Change journal trigger {trigger} on {table} is not on a single-key table of this target. It records only that table's own changes. Nothing drops it: if no other schema target in this database manages {table}, drop it"
                ),
            ));
        }
    }
    for (table, binding) in bindings {
        if let Some(message) = change_journal_binding_problem(live, table, binding) {
            diagnostics.push(rls_diagnostic(
                SchemaDiagnosticSeverity::Error,
                &table.table_name,
                message,
            ));
        }
    }
    diagnostics
}

/// Describe why `table` would not be journaled through its registry binding
/// and trigger, if it would not.
fn change_journal_binding_problem(
    live: &LiveRlsEnforcement,
    table: &ChangeJournalTable,
    binding: Option<&LiveChangeJournalBinding>,
) -> Option<String> {
    let name = &table.table_name;
    let Some(binding) = binding else {
        return Some(format!(
            "Missing change journal registry binding for {name}; re-apply the schema target"
        ));
    };
    let Some((schema, relation)) = binding.relation.as_ref() else {
        return Some(format!(
            "Change journal registry binding for {name} names a table that no longer exists; re-apply the schema target"
        ));
    };
    if binding.orm_relation.as_ref() != binding.relation.as_ref() {
        return Some(format!(
            "Change journal registry binds {name} to {}, but the ORM's writes resolve it to {}; re-apply the schema target",
            change_journal_table_key(schema, relation),
            binding
                .orm_relation
                .as_ref()
                .map(|(schema, relation)| change_journal_table_key(schema, relation))
                .unwrap_or_else(|| "no table".to_string())
        ));
    }
    if binding.entity_name != table.entity_name || binding.key_column != table.primary_key {
        return Some(format!(
            "Change journal registry binds {name} to entity {} by key {}, not {} by key {}; re-apply the schema target",
            binding.entity_name, binding.key_column, table.entity_name, table.primary_key
        ));
    }
    let Some(trigger) = live
        .change_journal_triggers
        .get(&change_journal_table_key(schema, relation))
    else {
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
        [journal, relation_schema, relation_name, rest @ ..]
            if Some(journal.as_str()) == live.change_journal_schema.as_deref()
                && *relation_schema == trigger.relation_schema
                && *relation_name == trigger.relation_name
                && rest.iter().map(String::as_str).eq(expected)
    );
    if !arguments_match || !trigger.journal_owned_by_function_owner {
        return Some(format!(
            "Change journal trigger {CHANGE_JOURNAL_TRIGGER} on {name} does not record {} by key {} into this target's {CHANGE_LOG_TABLE}, owned by the recorder's owner",
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
    let Some(trigger) = live.search_delete_triggers.get(&resolved_table_key(base)) else {
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
                && [table.clone()] == super::dialect::postgres_resolved_identifier_parts(&search_table_sql(search)).as_slice()
                && *key == super::dialect::postgres_resolved_identifier(&search.primary_key)
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

fn normalize_table_key(table_name: &str) -> String {
    let unquoted = unquote_identifier_path(table_name);
    unquoted
        .strip_prefix("public_")
        .unwrap_or(&unquoted)
        .to_string()
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

#[cfg(test)]
mod drift_tests {
    use super::*;

    static POLICIES: &[RlsOperationPolicy] = &[
        RlsOperationPolicy::generated(
            RlsOperation::Select,
            Some("docs.read"),
            Some("tenant_id"),
            None,
        ),
        RlsOperationPolicy::custom(RlsOperation::Delete, "status IN ('a', 'b')"),
    ];

    fn entity() -> RlsEntityMetadata {
        RlsEntityMetadata {
            entity_name: "Doc",
            table_name: "docs",
            force: true,
            policies: POLICIES,
        }
    }

    fn rls_target() -> RlsSchemaModel {
        RlsSchemaModel {
            entities: vec![entity()],
        }
    }

    /// A rendering stand-in: PostgreSQL would render both sides identically.
    fn rendered(expression: &str) -> RenderedRlsExpression {
        Ok(format!("rendered[{expression}]"))
    }

    fn select_predicate() -> String {
        render_policy_predicate(DatabaseBackend::Postgres, &POLICIES[0]).expect("predicate")
    }

    fn policy(name: &str, command: LiveRlsCommand, expression: &str) -> LiveRlsPolicyState {
        let clause = Some(rendered(expression));
        LiveRlsPolicyState {
            name: name.to_string(),
            command,
            permissive: true,
            roles: vec!["public".to_string()],
            using: (command != LiveRlsCommand::Insert)
                .then(|| clause.clone())
                .flatten(),
            check: matches!(command, LiveRlsCommand::Insert | LiveRlsCommand::Update)
                .then_some(clause)
                .flatten(),
        }
    }

    fn healthy() -> LiveRlsTarget {
        let select = select_predicate();
        let delete = "status IN ('a', 'b')".to_string();
        LiveRlsTarget {
            tables: vec![LiveRlsTableState {
                table_sql: "docs".to_string(),
                relation: Some(("public".to_string(), "docs".to_string())),
                enabled: true,
                forced: true,
                row_security_active: true,
                policies: vec![
                    policy("graphql_orm_docs_select", LiveRlsCommand::Select, &select),
                    policy("graphql_orm_docs_delete", LiveRlsCommand::Delete, &delete),
                ],
                expected: vec![
                    (select.clone(), rendered(&select)),
                    (delete.clone(), rendered(&delete)),
                ],
            }],
            helper_functions: RLS_HELPER_FUNCTIONS
                .iter()
                .map(|helper| LiveRlsHelperFunction {
                    name: helper.name.to_string(),
                    arguments: helper.arguments.to_string(),
                    returns: helper.returns.to_string(),
                    body: helper.body.to_string(),
                    language: "sql".to_string(),
                    volatility: "s".to_string(),
                    kind: "f".to_string(),
                    ..Default::default()
                })
                .collect(),
        }
    }

    fn messages(live: &LiveRlsTarget) -> Vec<String> {
        validate_rls_models(
            DatabaseBackend::Postgres,
            SchemaPolicy::Managed,
            live,
            &rls_target(),
        )
        .into_iter()
        .map(|diagnostic| {
            assert_eq!(diagnostic.severity, SchemaDiagnosticSeverity::Error);
            diagnostic.message
        })
        .collect()
    }

    fn assert_reported(live: &LiveRlsTarget, fragments: &[&str]) {
        let messages = messages(live);
        assert!(
            messages
                .iter()
                .any(|message| fragments.iter().all(|fragment| message.contains(fragment))),
            "{fragments:?}: {messages:#?}"
        );
    }

    #[test]
    fn the_plan_drops_policies_of_undeclared_operations() {
        let statements = render_postgres_entity_rls_sql(&entity());
        for operation in RLS_OPERATIONS {
            let name = policy_name("docs", operation);
            assert!(
                statements
                    .iter()
                    .any(|statement| statement
                        == &format!("DROP POLICY IF EXISTS \"{name}\" ON docs")),
                "{name}: {statements:#?}"
            );
        }
        let created = statements
            .iter()
            .filter(|statement| statement.starts_with("CREATE POLICY"))
            .count();
        assert_eq!(created, 2, "{statements:#?}");
    }

    #[test]
    fn generated_predicates_quote_one_identifier_and_group_each_condition() {
        let policy = RlsOperationPolicy::generated(
            RlsOperation::Select,
            Some("it's"),
            Some("\"tenant_id\" = \"tenant_id\" OR true OR \"tenant_id\""),
            Some("owner_id"),
        );
        assert_eq!(
            render_policy_predicate(DatabaseBackend::Postgres, &policy).as_deref(),
            Some(
                "(graphql_orm.has_scope('it''s')) AND (\"\"\"tenant_id\"\" = \"\"tenant_id\"\" OR true OR \"\"tenant_id\"\"\" = graphql_orm.current_tenant_id()) AND (owner_id = graphql_orm.current_user_id())"
            )
        );
    }

    #[test]
    fn quoted_columns_render_as_the_ddl_writes_them() {
        for (column, rendered) in [
            ("tenant_id", "tenant_id"),
            ("TenantId", "TenantId"),
            ("\"TenantId\"", "\"TenantId\""),
            ("\"a\"\"b\"", "\"a\"\"b\""),
            ("order", "\"order\""),
            (
                "\"a\" OR true OR \"b\"",
                "\"\"\"a\"\" OR true OR \"\"b\"\"\"",
            ),
            ("\"\"", "\"\"\"\"\"\""),
        ] {
            assert_eq!(quote_policy_column(column), rendered, "{column}");
        }
        assert_eq!(
            super::super::dialect::postgres_resolved_identifier("\"TenantId\""),
            "TenantId"
        );
        assert_eq!(
            super::super::dialect::postgres_resolved_identifier("\"a\" OR \"b\""),
            "\"a\" OR \"b\""
        );
    }

    #[test]
    fn matching_renderings_validate_clean() {
        assert_eq!(messages(&healthy()), Vec::<String>::new());
    }

    #[test]
    fn every_difference_is_an_error() {
        let mut differs = healthy();
        differs.tables[0].policies[0].using = Some(rendered("true"));
        assert_reported(
            &differs,
            &["graphql_orm_docs_select", "USING expression differs"],
        );

        let mut unrendered = healthy();
        unrendered.tables[0].expected[0].1 = Err("permission denied".to_string());
        assert_reported(
            &unrendered,
            &["graphql_orm_docs_select", "could not be compared"],
        );

        let mut live_unrendered = healthy();
        live_unrendered.tables[0].policies[1].using = Some(Err("syntax".to_string()));
        assert_reported(
            &live_unrendered,
            &["graphql_orm_docs_delete", "could not be compared"],
        );

        let mut unmatched = healthy();
        unmatched.tables[0].expected.clear();
        assert_reported(
            &unmatched,
            &["graphql_orm_docs_select", "could not be compared"],
        );

        let mut extra = healthy();
        extra.tables[0]
            .policies
            .push(policy("widen", LiveRlsCommand::All, "true"));
        assert_reported(&extra, &["Unexpected RLS policy widen", "FOR ALL"]);

        let mut stale = healthy();
        stale.tables[0].policies.push(policy(
            "graphql_orm_docs_update",
            LiveRlsCommand::Update,
            "true",
        ));
        assert_reported(&stale, &["graphql_orm_docs_update", "has not declared"]);

        let mut restrictive = healthy();
        restrictive.tables[0].policies[0].permissive = false;
        assert_reported(&restrictive, &["graphql_orm_docs_select", "RESTRICTIVE"]);

        let mut roles = healthy();
        roles.tables[0].policies[0].roles = vec!["app".to_string()];
        assert_reported(&roles, &["graphql_orm_docs_select", "applies to roles app"]);

        let mut command = healthy();
        command.tables[0].policies[1].command = LiveRlsCommand::All;
        assert_reported(
            &command,
            &["graphql_orm_docs_delete", "FOR ALL, not FOR DELETE"],
        );

        let mut missing = healthy();
        missing.tables[0].policies.remove(1);
        assert_reported(&missing, &["Missing RLS policy graphql_orm_docs_delete"]);

        let mut unresolved = healthy();
        unresolved.tables[0].relation = None;
        assert_reported(&unresolved, &["Missing RLS table state for docs"]);

        let mut disabled = healthy();
        disabled.tables[0].enabled = false;
        disabled.tables[0].forced = false;
        assert_reported(&disabled, &["RLS is not enabled on docs"]);
        assert_reported(&disabled, &["RLS is not forced on docs"]);

        let mut helper = healthy();
        helper.helper_functions[2].body = " SELECT 'tenant-a' ".to_string();
        assert_reported(&helper, &["graphql_orm.current_tenant_id()", "differs"]);
        let mut definer = healthy();
        definer.helper_functions[6].security_definer = true;
        assert_reported(&definer, &["graphql_orm.has_scope(scope text)", "differs"]);
        let mut absent = healthy();
        absent.helper_functions.remove(0);
        assert_reported(
            &absent,
            &["Missing RLS helper function graphql_orm.current_user_id()"],
        );
    }

    #[test]
    fn probes_cover_entity_and_search_tables_once() {
        let target = SchemaTarget {
            schema: super::super::core::SchemaModel {
                extensions: Vec::new(),
                tables: Vec::new(),
            },
            rls: RlsSchemaModel {
                entities: vec![entity(), entity()],
            },
        };
        let probes = rls_table_probes(&target);
        assert_eq!(probes.len(), 1, "{probes:#?}");
        assert_eq!(probes[0].table_sql, "docs");
        assert_eq!(
            probes[0]
                .expressions
                .iter()
                .map(|expression| expression.declared.clone())
                .collect::<Vec<_>>(),
            vec![select_predicate(), "status IN ('a', 'b')".to_string()]
        );
    }
}

#[cfg(test)]
mod identifier_path_tests {
    use super::super::dialect::identifier_path_is_bare;
    use super::change_journal_orm_path;

    /// The registry binds each declared path through the SQL text the ORM's
    /// own writes use, so PostgreSQL folds plain identifiers exactly as it
    /// does for those writes.
    #[test]
    fn declared_paths_render_as_the_orm_writes_them() {
        for (path, rendered, bare) in [
            ("rls_cov_docs", "rls_cov_docs", true),
            ("Users", "Users", true),
            ("App.users", "App.users", false),
            ("\"Users\"", "\"Users\"", true),
            ("app.\"Orders\"", "app.\"Orders\"", false),
            ("app.\"a.b\"", "app.\"a.b\"", false),
            ("user", "\"user\"", true),
        ] {
            assert_eq!(change_journal_orm_path(path), rendered, "{path}");
            assert_eq!(identifier_path_is_bare(path), bare, "{path}");
        }
    }
}

#[cfg(all(test, feature = "change-journal"))]
mod change_journal_tests {
    use super::*;

    fn live(owned: bool) -> LiveRlsEnforcement {
        LiveRlsEnforcement {
            role: "app_runtime".to_string(),
            change_journal_owned_by_current_role: owned,
            change_journal_registry_present: true,
            change_journal_registry_owned_by_recorder_owner: true,
            change_journal_function: Some(LiveChangeJournalFunction {
                body: CHANGE_JOURNAL_FUNCTION_BODY.to_string(),
                config: vec![CHANGE_JOURNAL_FUNCTION_CONFIG.to_string()],
                security_definer: true,
            }),
            ..Default::default()
        }
    }

    fn journal_table() -> LiveRlsTable {
        LiveRlsTable {
            table_name: CHANGE_LOG_TABLE.to_string(),
            enabled: true,
            forced: false,
            policies: Vec::new(),
        }
    }

    fn messages(live: &LiveRlsEnforcement) -> Vec<String> {
        validate_change_journal(
            DatabaseBackend::Postgres,
            SchemaPolicy::Managed,
            &[journal_table()],
            Some(live),
            &SchemaTarget::from_entities(&[], &[]),
        )
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect()
    }

    #[test]
    fn registry_integrity_is_reported() {
        assert!(messages(&live(false)).is_empty());
        let missing = LiveRlsEnforcement {
            change_journal_registry_present: false,
            ..live(false)
        };
        assert!(
            messages(&missing)
                .iter()
                .any(|message| message.contains("Missing change journal registry"))
        );
        let foreign = LiveRlsEnforcement {
            change_journal_registry_owned_by_recorder_owner: false,
            ..live(false)
        };
        assert!(
            messages(&foreign)
                .iter()
                .any(|message| message.contains("is not owned by the owner"))
        );
        let writable = LiveRlsEnforcement {
            change_journal_registry_writers: vec!["app_runtime".to_string()],
            ..live(false)
        };
        assert!(messages(&writable).iter().any(|message| {
            message.contains("Role app_runtime holds a privilege other than SELECT")
                && message.contains(CHANGE_JOURNAL_REGISTRY)
        }));
    }

    fn users() -> ChangeJournalTable {
        ChangeJournalTable {
            entity_name: "User".to_string(),
            table_name: "Users".to_string(),
            bare: true,
            primary_key: "id".to_string(),
            encoding: "text",
        }
    }

    fn binding() -> LiveChangeJournalBinding {
        LiveChangeJournalBinding {
            journal_schema: "public".to_string(),
            declared_table: "Users".to_string(),
            entity_name: "User".to_string(),
            key_column: "id".to_string(),
            relation: Some(("public".to_string(), "users".to_string())),
            orm_relation: Some(("public".to_string(), "users".to_string())),
        }
    }

    fn bound_live() -> LiveRlsEnforcement {
        let mut live = live(false);
        live.change_journal_schema = Some("public".to_string());
        live.change_journal_triggers.insert(
            change_journal_table_key("public", "users"),
            LiveChangeJournalTrigger {
                enabled: "O".to_string(),
                trigger_type: CHANGE_JOURNAL_TRIGGER_TYPE,
                conditional: false,
                calls_orm_function: true,
                function_owns_table: true,
                arguments: ["public", "public", "users", "User", "Users", "id", "text"]
                    .map(str::to_string)
                    .to_vec(),
                relation_schema: "public".to_string(),
                relation_name: "users".to_string(),
                journal_owned_by_function_owner: true,
            },
        );
        live
    }

    #[test]
    fn binding_problems_are_reported() {
        let live = bound_live();
        let problem = |binding: Option<&LiveChangeJournalBinding>, live: &LiveRlsEnforcement| {
            change_journal_binding_problem(live, &users(), binding)
        };
        assert_eq!(problem(Some(&binding()), &live), None);
        let expect = |outcome: Option<String>, text: &str| {
            assert!(
                outcome
                    .as_deref()
                    .is_some_and(|message| message.contains(text)),
                "{text}: {outcome:?}"
            );
        };
        expect(
            problem(None, &live),
            "Missing change journal registry binding",
        );
        expect(
            problem(
                Some(&LiveChangeJournalBinding {
                    relation: None,
                    ..binding()
                }),
                &live,
            ),
            "no longer exists",
        );
        expect(
            problem(
                Some(&LiveChangeJournalBinding {
                    relation: Some(("public".to_string(), "Users".to_string())),
                    ..binding()
                }),
                &live,
            ),
            "the ORM's writes resolve it to \"public\".\"users\"",
        );
        expect(
            problem(
                Some(&LiveChangeJournalBinding {
                    entity_name: "Admin".to_string(),
                    ..binding()
                }),
                &live,
            ),
            "to entity Admin by key id, not User by key id",
        );
        expect(
            problem(
                Some(&LiveChangeJournalBinding {
                    key_column: "email".to_string(),
                    ..binding()
                }),
                &live,
            ),
            "to entity User by key email",
        );
        let mut untriggered = live.clone();
        untriggered.change_journal_triggers.clear();
        expect(
            problem(Some(&binding()), &untriggered),
            "Missing change journal trigger",
        );
        let mut elsewhere = live.clone();
        elsewhere.change_journal_schema = Some("other".to_string());
        expect(
            problem(
                Some(&LiveChangeJournalBinding {
                    journal_schema: "other".to_string(),
                    ..binding()
                }),
                &elsewhere,
            ),
            "into this target's",
        );
    }

    #[test]
    fn a_role_acting_as_the_journal_owner_is_reported_without_rls_entities() {
        let target = SchemaTarget::from_entities(&[], &[]);
        assert!(target.rls.entities.is_empty());
        let journal = LiveRlsTable {
            table_name: CHANGE_LOG_TABLE.to_string(),
            enabled: true,
            forced: false,
            policies: Vec::new(),
        };
        let diagnostics = |owned| {
            validate_change_journal(
                DatabaseBackend::Postgres,
                SchemaPolicy::Managed,
                std::slice::from_ref(&journal),
                Some(&live(owned)),
                &target,
            )
        };
        let owned = diagnostics(true);
        assert_eq!(owned.len(), 1, "{owned:#?}");
        assert!(owned[0].message.contains("acts as the journal owner"));
        assert_eq!(owned[0].severity, SchemaDiagnosticSeverity::Error);
        assert!(diagnostics(false).is_empty());
    }
}
