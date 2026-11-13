//! Drift validation of generated and custom PostgreSQL RLS policies.
//!
//! A freshly applied target must validate clean: PostgreSQL re-renders policy
//! expressions (dropping identifier quotes, adding casts and parentheses), so
//! validation compares PostgreSQL's own rendering of the declared and the live
//! expressions. Every other difference must still be reported.
//!
//! Container-backed tests are ignored by default and run only against the
//! test-owned disposable PostgreSQL container.
#![cfg(feature = "postgres")]

#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use graphql_orm::graphql::orm::{
    ApplyOptions, DatabaseBackend, DatabaseRls, Entity, SchemaDiagnosticSeverity, SchemaPolicy,
    SchemaTarget, build_schema_target_rls_plan,
};
use graphql_orm::prelude::*;

/// Generated predicates for SELECT and INSERT; custom predicates with
/// parentheses, an `IN` list, casts, `current_setting(...)` and boolean
/// operators for UPDATE and DELETE.
#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    backend = "postgres",
    table = "rls_drift_docs",
    plural = "RlsDriftDocs"
)]
#[graphql_rls(
    select(scope = "docs.read", tenant = "tenant_id", owner = "owner_id"),
    insert(tenant = "tenant_id"),
    update(
        predicate = "(status IN ('draft', 'review') OR graphql_orm.has_scope('docs.admin')) AND tenant_id = graphql_orm.current_tenant_id()"
    ),
    delete(
        predicate = "current_setting('app.tenant_id', true) = tenant_id AND priority::text <> '0' AND NOT (status = 'locked')"
    )
)]
#[allow(dead_code)]
struct RlsDriftDoc {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    tenant_id: String,
    owner_id: String,
    status: String,
    priority: i32,
}

/// RLS entity in a non-public schema whose flattened name collides with
/// [`DriftAuditLogs`].
#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    backend = "postgres",
    table = "drift_audit.logs",
    plural = "DriftAuditLogEntries"
)]
#[graphql_rls(select(tenant = "tenant_id"), insert(tenant = "tenant_id"))]
#[allow(dead_code)]
struct DriftAuditLog {
    #[primary_key]
    id: String,
    tenant_id: String,
}

/// Plain public table named like the schema-qualified RLS table.
#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    backend = "postgres",
    table = "drift_audit_logs",
    plural = "DriftAuditLogs"
)]
#[allow(dead_code)]
struct DriftAuditLogs {
    #[primary_key]
    id: String,
}

/// RLS search entity whose unquoted mixed-case table PostgreSQL folds to
/// `driftnotes`, as the ORM's own DDL and writes do.
#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(backend = "postgres", table = "DriftNotes", plural = "DriftNotes")]
#[graphql_rls(
    select(tenant = "tenant_id"),
    insert(tenant = "tenant_id"),
    update(tenant = "tenant_id"),
    delete(tenant = "tenant_id")
)]
#[graphql_orm(search(index = true, language = "english"))]
#[allow(dead_code)]
struct DriftNote {
    #[primary_key]
    id: String,
    tenant_id: String,
    #[graphql_orm(searchable(weight = "A"))]
    body: String,
}

/// RLS entity whose tenant column is declared as a quoted identifier.
#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    backend = "postgres",
    table = "rls_drift_quoted",
    plural = "RlsDriftQuoteds"
)]
#[graphql_rls(select(tenant = "\"TenantId\""), insert(tenant = "\"TenantId\""))]
#[allow(dead_code)]
struct RlsDriftQuoted {
    #[primary_key]
    id: String,
    #[graphql_orm(db_column = "\"TenantId\"")]
    tenant: String,
}

/// RLS entity whose custom predicate calls a function without a schema.
/// Validation cannot compare it: names outside the catalog must be
/// schema-qualified.
#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    backend = "postgres",
    table = "rls_drift_members",
    plural = "RlsDriftMembers"
)]
#[graphql_rls(select(predicate = "is_member(tenant_id)"))]
#[allow(dead_code)]
struct RlsDriftMember {
    #[primary_key]
    id: String,
    tenant_id: String,
}

const DOCS: &str = "rls_drift_docs";
const LOGS: &str = "drift_audit.logs";

fn target() -> SchemaTarget {
    SchemaTarget::from_entities(
        &[
            <RlsDriftDoc as Entity>::metadata(),
            <DriftAuditLog as Entity>::metadata(),
            <DriftAuditLogs as Entity>::metadata(),
            <DriftNote as Entity>::metadata(),
            <RlsDriftQuoted as Entity>::metadata(),
        ],
        &[
            <RlsDriftDoc as DatabaseRls>::rls_metadata(),
            <DriftAuditLog as DatabaseRls>::rls_metadata(),
            <DriftAuditLogs as DatabaseRls>::rls_metadata(),
            <DriftNote as DatabaseRls>::rls_metadata(),
            <RlsDriftQuoted as DatabaseRls>::rls_metadata(),
        ],
    )
}

fn member_target() -> SchemaTarget {
    SchemaTarget::from_entities(
        &[<RlsDriftMember as Entity>::metadata()],
        &[<RlsDriftMember as DatabaseRls>::rls_metadata()],
    )
}

async fn apply(owner: &sqlx::PgPool) -> Result<(), Box<dyn std::error::Error>> {
    let database = graphql_orm::db::Database::new(owner.clone());
    let plan = database
        .schema()
        .plan_schema_target("1", "rls drift", &target())
        .await?;
    database
        .schema()
        .apply_schema_target(&plan, ApplyOptions::default())
        .await?;
    Ok(())
}

/// Re-run the target's RLS statements, as re-applying the target does.
async fn reapply_rls(owner: &sqlx::PgPool) -> Result<(), Box<dyn std::error::Error>> {
    let plan =
        build_schema_target_rls_plan(DatabaseBackend::Postgres, SchemaPolicy::Managed, &target());
    let mut tx = owner.begin().await?;
    for statement in &plan.statements {
        sqlx::raw_sql(statement).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Provision the target and a restricted runtime role; return both pools.
async fn provision() -> Result<(sqlx::PgPool, String, sqlx::PgPool), Box<dyn std::error::Error>> {
    let owner = owned_postgres::OwnedPostgres::pool("postgres-rls-drift", 5).await?;
    sqlx::query("CREATE SCHEMA drift_audit")
        .execute(&owner)
        .await?;
    apply(&owner).await?;
    let token = graphql_orm::uuid::Uuid::new_v4().simple().to_string();
    let role = format!("rls_drift_runtime_{token}");
    let password = format!("synthetic_{token}");
    for statement in [
        format!("CREATE ROLE {role} LOGIN NOSUPERUSER NOBYPASSRLS PASSWORD '{password}'"),
        format!("GRANT USAGE ON SCHEMA public, drift_audit, graphql_orm TO {role}"),
        format!(
            "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public, drift_audit TO {role}"
        ),
        // With the change-journal feature a runtime role may only read the journal.
        format!(
            "DO $$ BEGIN IF to_regclass('__graphql_orm_change_log') IS NOT NULL THEN REVOKE ALL ON __graphql_orm_change_log FROM {role}; GRANT SELECT ON __graphql_orm_change_log TO {role}; END IF; END $$"
        ),
    ] {
        sqlx::query(&statement).execute(&owner).await?;
    }
    let options = owner
        .connect_options()
        .as_ref()
        .clone()
        .username(&role)
        .password(&password);
    let runtime = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await?;
    Ok((owner, role, runtime))
}

/// A pool connecting as `role` whose sessions use `search_path`, as a runtime
/// role can set for itself.
async fn pool_with_search_path(
    owner: &sqlx::PgPool,
    role: &str,
    search_path: &str,
) -> Result<sqlx::PgPool, Box<dyn std::error::Error>> {
    let token = role.trim_start_matches("rls_drift_runtime_");
    let options = owner
        .connect_options()
        .as_ref()
        .clone()
        .username(role)
        .password(&format!("synthetic_{token}"))
        .options([("search_path", search_path)]);
    Ok(sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await?)
}

async fn rls_errors(pool: &sqlx::PgPool) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    rls_errors_for(pool, &target()).await
}

async fn rls_errors_for(
    pool: &sqlx::PgPool,
    target: &SchemaTarget,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let report = graphql_orm::db::Database::new(pool.clone())
        .schema()
        .validate_target(target)
        .await?;
    Ok(report
        .rls_diagnostics
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == SchemaDiagnosticSeverity::Error)
        .map(|diagnostic| {
            format!(
                "{}: {}",
                diagnostic.table.unwrap_or_default(),
                diagnostic.message
            )
        })
        .collect())
}

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn a_freshly_applied_target_reports_no_rls_drift() -> Result<(), Box<dyn std::error::Error>> {
    let (owner, _, runtime) = provision().await?;
    let errors = rls_errors(&runtime).await?;
    assert!(errors.is_empty(), "{errors:#?}");

    // The policies PostgreSQL stores are its own rendering, not the declared
    // text: quotes dropped, casts and parentheses added.
    let stored: String =
        sqlx::query_scalar("SELECT qual FROM pg_policies WHERE tablename = $1 AND policyname = $2")
            .bind(DOCS)
            .bind("graphql_orm_rls_drift_docs_select")
            .fetch_one(&owner)
            .await?;
    assert!(!stored.contains('"'), "{stored}");
    assert!(stored.contains("'docs.read'::text"), "{stored}");

    // The mixed-case entity's policies, search policies and delete trigger
    // bind to the folded tables the ORM writes.
    let bound: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM pg_policies WHERE schemaname = 'public' AND tablename IN ('driftnotes', '__graphql_orm_search_driftnotes'))
              + (SELECT COUNT(*) FROM pg_trigger WHERE tgrelid = 'public.driftnotes'::regclass AND tgname = 'graphql_orm_search_delete')",
    )
    .fetch_one(&owner)
    .await?;
    assert_eq!(bound, 9);

    // Re-applying the same target stays clean.
    reapply_rls(&owner).await?;
    let errors = rls_errors(&runtime).await?;
    assert!(errors.is_empty(), "{errors:#?}");
    Ok(())
}

/// Apply `drift` as the owner, assert that the runtime role's validation
/// reports an error containing every fragment, then re-apply the target.
/// Re-applying never drops a policy graphql-orm did not generate, so those
/// are dropped explicitly.
async fn assert_drift(
    owner: &sqlx::PgPool,
    runtime: &sqlx::PgPool,
    drift: &str,
    fragments: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    sqlx::raw_sql(drift).execute(owner).await?;
    let errors = rls_errors(runtime).await?;
    assert!(
        errors
            .iter()
            .any(|error| fragments.iter().all(|fragment| error.contains(fragment))),
        "{drift}\nexpected an error containing {fragments:?}: {errors:#?}"
    );
    reapply_rls(owner).await?;
    sqlx::raw_sql(&format!(
        "DROP POLICY IF EXISTS widen_reads ON {DOCS}; DROP POLICY IF EXISTS everything ON {DOCS}; DROP POLICY IF EXISTS widen_search ON __graphql_orm_search_driftnotes"
    ))
    .execute(owner)
    .await?;
    let errors = rls_errors(runtime).await?;
    assert!(
        errors.is_empty(),
        "after re-applying for {drift}: {errors:#?}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn real_rls_drift_is_still_reported() -> Result<(), Box<dyn std::error::Error>> {
    let (owner, role, runtime) = provision().await?;
    let select = "graphql_orm_rls_drift_docs_select";
    let update = "graphql_orm_rls_drift_docs_update";
    let delete = "graphql_orm_rls_drift_docs_delete";
    let cases: Vec<(String, Vec<String>)> = vec![
        // The same tokens, regrouped: `a OR (b AND c)` instead of `(a OR b) AND c`.
        (
            format!(
                "ALTER POLICY {update} ON {DOCS} USING (status IN ('draft', 'review') OR (graphql_orm.has_scope('docs.admin') AND tenant_id = graphql_orm.current_tenant_id()))"
            ),
            vec![update.into(), "differs".into()],
        ),
        // Only WITH CHECK changes.
        (
            format!(
                "ALTER POLICY {update} ON {DOCS} WITH CHECK ((status IN ('draft', 'review', 'published') OR graphql_orm.has_scope('docs.admin')) AND tenant_id = graphql_orm.current_tenant_id())"
            ),
            vec![update.into(), "differs".into()],
        ),
        // A widened predicate.
        (
            format!(
                "ALTER POLICY {select} ON {DOCS} USING (graphql_orm.has_scope('docs.read') AND tenant_id = graphql_orm.current_tenant_id() AND owner_id = graphql_orm.current_user_id() OR true)"
            ),
            vec![select.into(), "differs".into()],
        ),
        // A changed literal and a changed cast.
        (
            format!(
                "ALTER POLICY {delete} ON {DOCS} USING (current_setting('app.tenant_id', true) = tenant_id AND priority::text <> '1' AND NOT (status = 'locked'))"
            ),
            vec![delete.into(), "differs".into()],
        ),
        (
            format!(
                "ALTER POLICY {delete} ON {DOCS} USING (current_setting('app.tenant_id', true) = tenant_id AND priority::varchar <> '0' AND NOT (status = 'locked'))"
            ),
            vec![delete.into(), "differs".into()],
        ),
        // An additional permissive policy is OR-combined with the declared ones.
        (
            format!("CREATE POLICY widen_reads ON {DOCS} FOR SELECT USING (true)"),
            vec!["Unexpected RLS policy widen_reads".into()],
        ),
        // A FOR ALL policy.
        (
            format!("CREATE POLICY everything ON {DOCS} USING (true) WITH CHECK (true)"),
            vec!["Unexpected RLS policy everything".into(), "FOR ALL".into()],
        ),
        // The declared policy limited to one role.
        (
            format!("ALTER POLICY {select} ON {DOCS} TO {role}"),
            vec![select.into(), "applies to roles".into()],
        ),
        // The declared policy replaced by a restrictive one.
        (
            format!(
                "DROP POLICY {select} ON {DOCS}; CREATE POLICY {select} ON {DOCS} AS RESTRICTIVE FOR SELECT USING (graphql_orm.has_scope('docs.read') AND tenant_id = graphql_orm.current_tenant_id() AND owner_id = graphql_orm.current_user_id())"
            ),
            vec![select.into(), "RESTRICTIVE".into()],
        ),
        // The declared policy under another command.
        (
            format!(
                "DROP POLICY {delete} ON {DOCS}; CREATE POLICY {delete} ON {DOCS} FOR ALL USING (current_setting('app.tenant_id', true) = tenant_id AND priority::text <> '0' AND NOT (status = 'locked'))"
            ),
            vec![delete.into(), "FOR ALL".into()],
        ),
        // A missing policy.
        (
            format!("DROP POLICY {update} ON {DOCS}"),
            vec![format!("Missing RLS policy {update}")],
        ),
        // A generated policy name for an operation that is not declared.
        (
            format!(
                "CREATE POLICY graphql_orm_drift_audit_logs_delete ON {LOGS} FOR DELETE USING (true)"
            ),
            vec![
                "Unexpected RLS policy graphql_orm_drift_audit_logs_delete".into(),
                "not declared".into(),
            ],
        ),
        // A widened search-document policy on the mixed-case entity.
        (
            "ALTER POLICY graphql_orm_graphql_orm_search_driftnotes_select ON __graphql_orm_search_driftnotes USING (true)".to_string(),
            vec!["graphql_orm_graphql_orm_search_driftnotes_select".into(), "differs".into()],
        ),
        // RLS disabled on the schema-qualified table, not on its public namesake.
        (
            format!("ALTER TABLE {LOGS} DISABLE ROW LEVEL SECURITY"),
            vec![format!("RLS is not enabled on {LOGS}")],
        ),
        // An additional policy on a search-document table.
        (
            "CREATE POLICY widen_search ON __graphql_orm_search_driftnotes FOR SELECT USING (true)".to_string(),
            vec!["Unexpected RLS policy widen_search".into()],
        ),
        // A helper function with another volatility or a SET option.
        (
            "ALTER FUNCTION graphql_orm.current_user_id() VOLATILE".to_string(),
            vec!["graphql_orm.current_user_id".into(), "differs".into()],
        ),
        (
            "ALTER FUNCTION graphql_orm.claims() SET search_path = public".to_string(),
            vec!["graphql_orm.claims".into(), "differs".into()],
        ),
        // A helper function that no longer reads the request context.
        (
            "CREATE OR REPLACE FUNCTION graphql_orm.current_tenant_id() RETURNS text LANGUAGE sql STABLE AS $$ SELECT 'tenant-a'::text $$".to_string(),
            vec!["graphql_orm.current_tenant_id".into(), "differs".into()],
        ),
        (
            "ALTER FUNCTION graphql_orm.has_scope(text) SECURITY DEFINER".to_string(),
            vec!["graphql_orm.has_scope".into(), "differs".into()],
        ),
    ];
    for (drift, fragments) in &cases {
        let fragments = fragments.iter().map(String::as_str).collect::<Vec<_>>();
        assert_drift(&owner, &runtime, drift, &fragments).await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn validation_fails_closed_when_expressions_cannot_be_rendered()
-> Result<(), Box<dyn std::error::Error>> {
    let (owner, role, runtime) = provision().await?;
    // Without TEMP the runtime role cannot render expressions; validation
    // must report that rather than treat the policies as matching.
    sqlx::raw_sql(&format!(
        "REVOKE TEMP ON DATABASE {database} FROM PUBLIC; REVOKE TEMP ON DATABASE {database} FROM {role}",
        database = sqlx::query_scalar::<_, String>("SELECT quote_ident(current_database())")
            .fetch_one(&owner)
            .await?,
    ))
    .execute(&owner)
    .await?;
    let errors = rls_errors(&runtime).await?;
    assert!(
        errors
            .iter()
            .any(|error| error.contains("graphql_orm_rls_drift_docs_select")
                && error.contains("could not be compared")),
        "{errors:#?}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn row_level_security_refusals_are_forbidden() -> Result<(), Box<dyn std::error::Error>> {
    // The owner pool keeps the owned container alive.
    let (_owner, _, runtime) = provision().await?;
    // Without a tenant in the request context the generated WITH CHECK fails.
    let refused = sqlx::query(
        "INSERT INTO rls_drift_docs (id, tenant_id, owner_id, status, priority) VALUES (gen_random_uuid(), 'tenant-a', 'user-a', 'draft', 1)",
    )
    .execute(&runtime)
    .await
    .expect_err("RLS refuses the row");
    assert_eq!(
        refused
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("42501")
    );
    let public = graphql_orm::graphql::errors::OrmPublicError::from_sqlx(&refused);
    assert_eq!(
        public.code,
        graphql_orm::graphql::errors::OrmErrorCode::Forbidden
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn a_hostile_search_path_cannot_mask_a_lookalike_operator()
-> Result<(), Box<dyn std::error::Error>> {
    let (owner, role, _runtime) = provision().await?;
    // A lookalike `=` in the writable public schema, and the declared INSERT
    // policy rebound to it.
    sqlx::raw_sql(&format!(
        "CREATE FUNCTION public.always_equal(text, text) RETURNS boolean LANGUAGE sql IMMUTABLE AS 'SELECT true'; \
         CREATE OPERATOR public.= (LEFTARG = text, RIGHTARG = text, FUNCTION = public.always_equal); \
         ALTER POLICY graphql_orm_rls_drift_docs_insert ON {DOCS} WITH CHECK (tenant_id OPERATOR(public.=) graphql_orm.current_tenant_id())"
    ))
    .execute(&owner)
    .await?;
    // A runtime session that searches public before the catalog resolves the
    // declared `=` to the lookalike too.
    let hostile = pool_with_search_path(&owner, &role, "public,pg_catalog").await?;
    let errors = rls_errors(&hostile).await?;
    assert!(
        errors.iter().any(|error| {
            error.contains("graphql_orm_rls_drift_docs_insert") && error.contains("differs")
        }),
        "{errors:#?}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn a_hostile_search_path_cannot_mask_a_lookalike_function()
-> Result<(), Box<dyn std::error::Error>> {
    let (owner, role, runtime) = provision().await?;
    let select = "graphql_orm_rls_drift_members_select";
    sqlx::raw_sql(&format!(
        "CREATE FUNCTION public.is_member(text) RETURNS boolean LANGUAGE sql STABLE AS 'SELECT $1 = graphql_orm.current_tenant_id()'; \
         CREATE TABLE rls_drift_members (id text PRIMARY KEY, tenant_id text NOT NULL); \
         GRANT SELECT ON rls_drift_members TO {role}"
    ))
    .execute(&owner)
    .await?;
    let plan = build_schema_target_rls_plan(
        DatabaseBackend::Postgres,
        SchemaPolicy::Managed,
        &member_target(),
    );
    for statement in &plan.statements {
        sqlx::raw_sql(statement).execute(&owner).await?;
    }
    // Rebound to a lookalike in another schema, and validated by a session
    // whose search_path resolves the declared name to that lookalike, the
    // policy is still reported.
    sqlx::raw_sql(&format!(
        "CREATE SCHEMA lookalike; GRANT USAGE ON SCHEMA lookalike TO {role}; \
         CREATE FUNCTION lookalike.is_member(text) RETURNS boolean LANGUAGE sql STABLE AS 'SELECT true'; \
         ALTER POLICY {select} ON rls_drift_members USING (lookalike.is_member(tenant_id))"
    ))
    .execute(&owner)
    .await?;
    let hostile = pool_with_search_path(&owner, &role, "lookalike,public").await?;
    let errors = rls_errors_for(&hostile, &member_target()).await?;
    assert!(
        errors.iter().any(|error| error.contains(select)),
        "{errors:#?}"
    );
    // With any search_path, a predicate naming a function outside the catalog
    // without its schema cannot be compared, and is reported rather than
    // accepted.
    let errors = rls_errors_for(&runtime, &member_target()).await?;
    assert!(
        errors
            .iter()
            .any(|error| error.contains(select) && error.contains("could not be compared")),
        "{errors:#?}"
    );
    Ok(())
}

/// Generated policies on tenant and owner columns whose types live outside
/// the catalog: `citext` from an extension, and a domain.
#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    backend = "postgres",
    table = "rls_drift_ext_docs",
    plural = "RlsDriftExtDocs"
)]
#[graphql_rls(
    select(tenant = "tenant_id", owner = "owner_id"),
    insert(tenant = "tenant_id"),
    update(tenant = "tenant_id", owner = "owner_id"),
    delete(owner = "owner_id")
)]
#[allow(dead_code)]
struct RlsDriftExtDoc {
    #[primary_key]
    id: String,
    tenant_id: String,
    owner_id: String,
}

/// Generated policy on a tenant column whose equality with `text` resolves to
/// an operator outside the catalog.
#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    backend = "postgres",
    table = "rls_drift_ext_codes",
    plural = "RlsDriftExtCodes"
)]
#[graphql_rls(select(tenant = "tenant_id"))]
#[allow(dead_code)]
struct RlsDriftExtCode {
    #[primary_key]
    id: String,
    tenant_id: String,
}

/// The same tenant column with a custom predicate that names the operator
/// and the type with their schema.
#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    backend = "postgres",
    table = "rls_drift_ext_qualified",
    plural = "RlsDriftExtQualifieds"
)]
#[graphql_rls(select(
    predicate = "tenant_id OPERATOR(drift_ext.=) graphql_orm.current_tenant_id()::drift_ext.tenant_kind"
))]
#[allow(dead_code)]
struct RlsDriftExtQualified {
    #[primary_key]
    id: String,
    tenant_id: String,
}

fn ext_target() -> SchemaTarget {
    SchemaTarget::from_entities(
        &[
            <RlsDriftExtDoc as Entity>::metadata(),
            <RlsDriftExtCode as Entity>::metadata(),
            <RlsDriftExtQualified as Entity>::metadata(),
        ],
        &[
            <RlsDriftExtDoc as DatabaseRls>::rls_metadata(),
            <RlsDriftExtCode as DatabaseRls>::rls_metadata(),
            <RlsDriftExtQualified as DatabaseRls>::rls_metadata(),
        ],
    )
}

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn extension_type_tenant_columns_validate_as_postgresql_bound_them()
-> Result<(), Box<dyn std::error::Error>> {
    let (owner, role, runtime) = provision().await?;
    // The extension and the types live in their own schema, which the
    // applying session has on its path.
    sqlx::raw_sql(&format!(
        "CREATE SCHEMA drift_ext; GRANT USAGE ON SCHEMA drift_ext TO {role}; \
         CREATE EXTENSION citext SCHEMA drift_ext; \
         CREATE DOMAIN drift_ext.owner_code AS text; \
         CREATE TYPE drift_ext.tenant_kind AS ENUM ('tenant-a', 'tenant-b'); \
         CREATE FUNCTION drift_ext.tenant_kind_eq(drift_ext.tenant_kind, drift_ext.tenant_kind) RETURNS boolean LANGUAGE sql IMMUTABLE AS 'SELECT $1::text = $2::text'; \
         CREATE OPERATOR drift_ext.= (LEFTARG = drift_ext.tenant_kind, RIGHTARG = drift_ext.tenant_kind, FUNCTION = drift_ext.tenant_kind_eq); \
         CREATE CAST (text AS drift_ext.tenant_kind) WITH INOUT AS IMPLICIT; \
         CREATE TABLE rls_drift_ext_docs (id text PRIMARY KEY, tenant_id drift_ext.citext NOT NULL, owner_id drift_ext.owner_code NOT NULL); \
         CREATE TABLE rls_drift_ext_codes (id text PRIMARY KEY, tenant_id drift_ext.tenant_kind NOT NULL); \
         CREATE TABLE rls_drift_ext_qualified (id text PRIMARY KEY, tenant_id drift_ext.tenant_kind NOT NULL); \
         GRANT SELECT, INSERT, UPDATE, DELETE ON rls_drift_ext_docs, rls_drift_ext_codes, rls_drift_ext_qualified TO {role}"
    ))
    .execute(&owner)
    .await?;
    let plan = build_schema_target_rls_plan(
        DatabaseBackend::Postgres,
        SchemaPolicy::Managed,
        &ext_target(),
    );
    let mut tx = owner.begin().await?;
    sqlx::raw_sql("SET LOCAL search_path = public, drift_ext")
        .execute(&mut *tx)
        .await?;
    for statement in &plan.statements {
        sqlx::raw_sql(statement).execute(&mut *tx).await?;
    }
    tx.commit().await?;

    // PostgreSQL binds `citext = text` and `domain = text` to the catalog's
    // text equality, so these policies validate clean whatever the path.
    let on_path = pool_with_search_path(&owner, &role, "public,drift_ext").await?;
    for pool in [&runtime, &on_path] {
        let errors = rls_errors_for(pool, &ext_target()).await?;
        assert!(
            !errors
                .iter()
                .any(|error| error.starts_with("rls_drift_ext_docs:")),
            "{errors:#?}"
        );
        // A generated predicate whose `=` resolved to an operator outside the
        // catalog cannot be compared with only the catalog on the path: it is
        // reported, never accepted.
        assert!(
            errors.iter().any(|error| {
                error.starts_with("rls_drift_ext_codes:")
                    && error.contains("graphql_orm_rls_drift_ext_codes_select")
                    && error.contains("could not be compared")
            }),
            "{errors:#?}"
        );
        // Naming the operator and the type with their schema compares.
        assert!(
            !errors
                .iter()
                .any(|error| error.starts_with("rls_drift_ext_qualified:")),
            "{errors:#?}"
        );
    }
    let stored: String =
        sqlx::query_scalar("SELECT qual FROM pg_policies WHERE tablename = $1 AND policyname = $2")
            .bind("rls_drift_ext_docs")
            .bind("graphql_orm_rls_drift_ext_docs_select")
            .fetch_one(&owner)
            .await?;
    assert!(stored.contains("(tenant_id)::text ="), "{stored}");
    Ok(())
}

/// RLS search entity in a non-public schema whose flattened name equals the
/// plain public table `drift_audit_notes`.
#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    backend = "postgres",
    table = "drift_audit.notes",
    plural = "DriftAuditNotes"
)]
#[graphql_rls(select(tenant = "tenant_id"))]
#[graphql_orm(search(index = true, language = "english"))]
#[allow(dead_code)]
struct DriftAuditNote {
    #[primary_key]
    id: String,
    tenant_id: String,
    #[graphql_orm(searchable(weight = "A"))]
    body: String,
}

fn qualified_search_target() -> SchemaTarget {
    SchemaTarget::from_entities(
        &[<DriftAuditNote as Entity>::metadata()],
        &[<DriftAuditNote as DatabaseRls>::rls_metadata()],
    )
}

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn a_search_delete_trigger_counts_only_on_the_resolved_base_table()
-> Result<(), Box<dyn std::error::Error>> {
    let (owner, role, runtime) = provision().await?;
    let search_table = "__graphql_orm_search_drift_audit_notes";
    sqlx::raw_sql(&format!(
        "CREATE TABLE drift_audit.notes (id text PRIMARY KEY, tenant_id text NOT NULL, body text NOT NULL); \
         CREATE TABLE {search_table} (entity_pk text PRIMARY KEY); \
         CREATE TABLE public.drift_audit_notes (id text PRIMARY KEY); \
         GRANT SELECT, INSERT, UPDATE, DELETE ON drift_audit.notes, {search_table}, public.drift_audit_notes TO {role}"
    ))
    .execute(&owner)
    .await?;
    let plan = build_schema_target_rls_plan(
        DatabaseBackend::Postgres,
        SchemaPolicy::Managed,
        &qualified_search_target(),
    );
    assert!(
        plan.statements
            .iter()
            .any(|statement| statement.contains(search_table)),
        "{:#?}",
        plan.statements
    );
    for statement in &plan.statements {
        sqlx::raw_sql(statement).execute(&owner).await?;
    }
    let trigger_errors = || async {
        Ok::<_, Box<dyn std::error::Error>>(
            rls_errors_for(&runtime, &qualified_search_target())
                .await?
                .into_iter()
                .filter(|error| error.contains("graphql_orm_search_delete"))
                .collect::<Vec<_>>(),
        )
    };
    let applied = trigger_errors().await?;
    assert!(applied.is_empty(), "{applied:#?}");
    // Move the intact trigger to the public table whose flattened
    // `schema_table` name is the same.
    sqlx::raw_sql(&format!(
        "DROP TRIGGER graphql_orm_search_delete ON drift_audit.notes; \
         CREATE TRIGGER graphql_orm_search_delete BEFORE DELETE ON public.drift_audit_notes \
             FOR EACH ROW EXECUTE FUNCTION graphql_orm.delete_search_document('public', '{search_table}', 'id', 'text')"
    ))
    .execute(&owner)
    .await?;
    let moved = trigger_errors().await?;
    assert!(
        moved.iter().any(|error| error.contains(
            "Missing search document delete trigger graphql_orm_search_delete on drift_audit.notes"
        )),
        "{moved:#?}"
    );
    Ok(())
}

/// Append-only storage on an entity the schema half of validation reads.
#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    backend = "postgres",
    table = "drift_ledger",
    plural = "DriftLedgers",
    append_only = true
)]
#[allow(dead_code)]
struct DriftLedger {
    #[primary_key]
    id: String,
    note: String,
}

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn schema_introspection_reads_catalog_relations_under_any_search_path()
-> Result<(), Box<dyn std::error::Error>> {
    let owner = owned_postgres::OwnedPostgres::pool("postgres-catalog-relations", 5).await?;
    let database = graphql_orm::db::Database::new(owner.clone());
    let target = SchemaTarget::from_entities(&[<DriftLedger as Entity>::metadata()], &[]);
    let plan = database
        .schema()
        .plan_schema_target("1", "append-only ledger", &target)
        .await?;
    database
        .schema()
        .apply_schema_target(&plan, ApplyOptions::default())
        .await?;
    let append_only = |pool: sqlx::PgPool| async move {
        let model = graphql_orm::graphql::orm::introspect_schema::<
            graphql_orm::graphql::orm::PostgresBackend,
            _,
        >(&graphql_orm::db::Database::new(pool))
        .await?;
        Ok::<_, Box<dyn std::error::Error>>(
            model
                .tables
                .iter()
                .find(|table| table.table_name == "drift_ledger")
                .map(|table| table.append_only),
        )
    };
    assert_eq!(append_only(owner.clone()).await?, Some(true));
    // Remove the append-only trigger, and add a same-named `pg_trigger` view
    // in public that a session searching public first can see.
    sqlx::raw_sql(
        "CREATE TABLE public.saved_trigger AS SELECT * FROM pg_catalog.pg_trigger t \
             WHERE t.tgrelid OPERATOR(pg_catalog.=) 'public.drift_ledger'::pg_catalog.regclass::pg_catalog.oid AND NOT t.tgisinternal; \
         DO $$ DECLARE name pg_catalog.text; BEGIN SELECT tgname INTO name FROM public.saved_trigger; \
             EXECUTE pg_catalog.format('DROP TRIGGER %I ON public.drift_ledger', name); END $$; \
         CREATE VIEW public.pg_trigger AS SELECT * FROM pg_catalog.pg_trigger UNION ALL SELECT * FROM public.saved_trigger",
    )
    .execute(&owner)
    .await?;
    let options = owner
        .connect_options()
        .as_ref()
        .clone()
        .options([("search_path", "public,pg_catalog")]);
    let public_first = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await?;
    assert_eq!(append_only(owner.clone()).await?, Some(false));
    assert_eq!(append_only(public_first).await?, Some(false));
    Ok(())
}

/// A second append-only entity that keeps its trigger while
/// [`DriftLedger`]'s is removed.
#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    backend = "postgres",
    table = "drift_journal",
    plural = "DriftJournals",
    append_only = true
)]
#[allow(dead_code)]
struct DriftJournal {
    #[primary_key]
    id: String,
    note: String,
}

/// A pool for `owner`'s role whose sessions use `search_path`.
async fn owner_with_search_path(
    owner: &sqlx::PgPool,
    search_path: &str,
) -> Result<sqlx::PgPool, Box<dyn std::error::Error>> {
    let options = owner
        .connect_options()
        .as_ref()
        .clone()
        .options([("search_path", search_path)]);
    Ok(sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await?)
}

async fn introspected(
    pool: &sqlx::PgPool,
) -> Result<graphql_orm::graphql::orm::SchemaModel, Box<dyn std::error::Error>> {
    Ok(graphql_orm::graphql::orm::introspect_schema::<
        graphql_orm::graphql::orm::PostgresBackend,
        _,
    >(&graphql_orm::db::Database::new(pool.clone()))
    .await?)
}

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn schema_introspection_uses_catalog_operators_under_any_search_path()
-> Result<(), Box<dyn std::error::Error>> {
    let owner = owned_postgres::OwnedPostgres::pool("postgres-catalog-operators", 5).await?;
    let database = graphql_orm::db::Database::new(owner.clone());
    let target = SchemaTarget::from_entities(
        &[
            <DriftLedger as Entity>::metadata(),
            <DriftJournal as Entity>::metadata(),
        ],
        &[],
    );
    let plan = database
        .schema()
        .plan_schema_target("1", "append-only ledgers", &target)
        .await?;
    database
        .schema()
        .apply_schema_target(&plan, ApplyOptions::default())
        .await?;
    // Remove the ledger's append-only trigger; the journal keeps its own.
    // Operators named `=` for (name, text) and (text, text) in public are on
    // the second session's path; one partial index uses the latter, and
    // another uses an operator class defined in public.
    sqlx::raw_sql(
        "DO $$ DECLARE name pg_catalog.text; BEGIN \
             SELECT t.tgname INTO name FROM pg_catalog.pg_trigger t \
             WHERE t.tgrelid OPERATOR(pg_catalog.=) 'public.drift_ledger'::pg_catalog.regclass::pg_catalog.oid AND NOT t.tgisinternal; \
             EXECUTE pg_catalog.format('DROP TRIGGER %I ON public.drift_ledger', name); END $$; \
         CREATE TABLE public.drift_tags (id text PRIMARY KEY, status text NOT NULL); \
         CREATE FUNCTION public.any_name(name, text) RETURNS boolean LANGUAGE sql IMMUTABLE AS 'SELECT true'; \
         CREATE OPERATOR public.= (LEFTARG = name, RIGHTARG = text, FUNCTION = public.any_name); \
         CREATE FUNCTION public.loose_text(text, text) RETURNS boolean LANGUAGE sql IMMUTABLE AS 'SELECT pg_catalog.lower($1) OPERATOR(pg_catalog.=) pg_catalog.lower($2)'; \
         CREATE OPERATOR public.= (LEFTARG = text, RIGHTARG = text, FUNCTION = public.loose_text); \
         CREATE INDEX drift_tags_catalog ON public.drift_tags (id) WHERE status OPERATOR(pg_catalog.=) ANY (ARRAY['open', 'held']); \
         CREATE INDEX drift_tags_public_operator ON public.drift_tags (status) WHERE status OPERATOR(public.=) ANY (ARRAY['open', 'held']); \
         CREATE OPERATOR CLASS public.text_alt_ops FOR TYPE text USING btree AS \
             OPERATOR 1 pg_catalog.<, OPERATOR 2 pg_catalog.<=, OPERATOR 3 pg_catalog.=, \
             OPERATOR 4 pg_catalog.>=, OPERATOR 5 pg_catalog.>, \
             FUNCTION 1 pg_catalog.bttextcmp(text, text); \
         CREATE INDEX drift_tags_public_class ON public.drift_tags (status public.text_alt_ops) WHERE status OPERATOR(pg_catalog.=) ANY (ARRAY['open', 'held'])",
    )
    .execute(&owner)
    .await?;
    let public_first = owner_with_search_path(&owner, "public,pg_catalog").await?;
    for pool in [&owner, &public_first] {
        let model = introspected(pool).await?;
        let append_only = |table: &str| {
            model
                .tables
                .iter()
                .find(|candidate| candidate.table_name == table)
                .map(|candidate| candidate.append_only)
        };
        assert_eq!(append_only("drift_ledger"), Some(false));
        assert_eq!(append_only("drift_journal"), Some(true));
        let tags = model
            .tables
            .iter()
            .find(|candidate| candidate.table_name == "drift_tags")
            .expect("drift_tags is introspected");
        let predicate = |index: &str| {
            tags.indexes
                .iter()
                .find(|candidate| candidate.name == index)
                .map(|candidate| candidate.predicate.is_some())
        };
        // A closed-set predicate is reported only for the catalog's equality.
        // A session whose path puts another `=(text, text)` first sees the
        // catalog's rendered schema-qualified, which is reported without a
        // predicate rather than accepted.
        assert_eq!(
            predicate("drift_tags_catalog"),
            Some(std::ptr::eq(pool, &owner))
        );
        assert_eq!(predicate("drift_tags_public_operator"), Some(false));
        // So is one whose operator class is defined outside the catalog,
        // even with the catalog's equality in its predicate.
        assert_eq!(predicate("drift_tags_public_class"), Some(false));
    }
    Ok(())
}
