//! Regression coverage for PostgreSQL RLS enforcement gaps (GHSA-hr7v-hr83-2jj4,
//! GHSA-7gw2-974x-qhrh).
//!
//! Container-backed tests are ignored by default and run only against the
//! test-owned disposable PostgreSQL container. Assertions about tenant
//! isolation always execute as a separate restricted role: the container
//! owner is a superuser and cannot establish RLS enforcement.
#![cfg(feature = "postgres")]

#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use async_graphql::{Request, Schema};
use graphql_orm::graphql::auth::{AuthorizationMode, SystemAccess};
use graphql_orm::graphql::orm::{
    ApplyOptions, DatabaseBackend, DbAuthContext, PostgresBackend, SchemaDiagnostic,
    SchemaDiagnosticSeverity, SchemaPolicy, apply_db_auth_context_to_transaction,
    build_schema_target_rls_plan, fetch_rows_with_auth,
};
use graphql_orm::prelude::*;
use sqlx::Row;

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
    PartialEq,
)]
#[graphql_entity(
    backend = "postgres",
    table = "rls_cov_docs",
    plural = "RlsCovDocs",
    default_sort = "id ASC"
)]
#[graphql_rls(
    select(tenant = "tenant_id"),
    insert(tenant = "tenant_id"),
    update(tenant = "tenant_id"),
    delete(tenant = "tenant_id")
)]
#[graphql_orm(search(index = true, language = "english"))]
struct RlsCovDoc {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    pub tenant_id: String,

    #[graphql_orm(searchable(weight = "A"))]
    #[sortable]
    pub title: String,

    #[graphql_orm(searchable(weight = "B"))]
    pub body: Option<String>,
}

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
    PartialEq,
)]
#[graphql_entity(
    backend = "postgres",
    table = "rls_cov_loose",
    plural = "RlsCovLooses",
    default_sort = "id ASC"
)]
#[graphql_rls(force = false, select(tenant = "tenant_id"))]
struct RlsCovLoose {
    #[primary_key]
    #[sortable]
    pub id: String,

    #[filterable(type = "string")]
    pub tenant_id: String,
}

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
    PartialEq,
)]
#[graphql_entity(
    backend = "postgres",
    table = "rls_cov_plain",
    plural = "RlsCovPlains",
    default_sort = "id ASC"
)]
struct RlsCovPlain {
    #[primary_key]
    pub id: String,

    #[sortable]
    pub label: String,
}

/// Non-RLS parent whose relation filter, count ordering and dependent delete
/// reach the RLS `RlsCovFile` children.
#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    async_graphql::SimpleObject,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
    PartialEq,
)]
#[graphql_entity(
    backend = "postgres",
    dependent_deletes,
    table = "rls_cov_folders",
    plural = "RlsCovFolders",
    default_sort = "id ASC"
)]
#[graphql(complex)]
struct RlsCovFolder {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    pub id: String,

    pub label: String,

    #[graphql(skip)]
    #[relation(
        target = "RlsCovFile",
        from = "id",
        to = "folder_id",
        multiple,
        filter,
        order_aggregate(name = "FileCount", aggregate = "count")
    )]
    pub files: Vec<RlsCovFile>,
}

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    async_graphql::SimpleObject,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
    PartialEq,
)]
#[graphql_entity(
    backend = "postgres",
    dependent_deletes,
    table = "rls_cov_files",
    plural = "RlsCovFiles",
    default_sort = "id ASC"
)]
#[graphql_rls(
    select(tenant = "tenant_id"),
    insert(tenant = "tenant_id"),
    update(tenant = "tenant_id"),
    delete(tenant = "tenant_id")
)]
#[graphql_orm(search(index = true, language = "english"))]
#[graphql(complex)]
struct RlsCovFile {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    pub id: String,

    #[filterable(type = "string")]
    pub tenant_id: String,

    #[filterable(type = "string")]
    pub folder_id: String,

    #[filterable(type = "string")]
    #[graphql_orm(searchable(weight = "A"))]
    pub note: String,

    #[graphql(skip)]
    #[relation(
        target = "RlsCovFolder",
        from = "folder_id",
        to = "id",
        on_delete = "cascade"
    )]
    pub folder: Option<RlsCovFolder>,
}

impl BatchLoadEntity for RlsCovFolder {
    fn batch_column() -> &'static str {
        "id"
    }

    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, sqlx::Error> {
        sqlx::Row::try_get(row, "id")
    }
}

impl BatchLoadEntity for RlsCovFile {
    fn batch_column() -> &'static str {
        "folder_id"
    }

    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, sqlx::Error> {
        sqlx::Row::try_get(row, "folder_id")
    }
}

/// RLS search entity with a binary primary key (`bin:<hex>` document keys).
#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
    PartialEq,
)]
#[graphql_entity(
    backend = "postgres",
    table = "rls_cov_blobs",
    plural = "RlsCovBlobs",
    default_sort = "body ASC"
)]
#[graphql_rls(
    select(tenant = "tenant_id"),
    insert(tenant = "tenant_id"),
    update(tenant = "tenant_id"),
    delete(tenant = "tenant_id")
)]
#[graphql_orm(search(index = true, language = "english"))]
struct RlsCovBlob {
    #[primary_key]
    #[filterable(type = "bytes")]
    #[graphql_orm(skip_input, auto_generated = false)]
    pub digest: Vec<u8>,

    #[filterable(type = "string")]
    pub tenant_id: String,

    #[sortable]
    #[graphql_orm(searchable(weight = "A"))]
    pub body: String,
}

schema_roots! {
    backend: "postgres",
    query_custom_ops: [],
    entities: [RlsCovDoc, RlsCovLoose, RlsCovPlain, RlsCovFolder, RlsCovFile, RlsCovBlob],
}

type CoverageSchema = Schema<QueryRoot, MutationRoot, SubscriptionRoot>;

const SEARCH_TABLE: &str = "__graphql_orm_search_rls_cov_docs";
const CHANGE_LOG_TABLE: &str = "__graphql_orm_change_log";

fn tenant(tenant_id: &str) -> DbAuthContext {
    DbAuthContext {
        user_id: Some(format!("user-{tenant_id}")),
        subject: Some(format!("user-{tenant_id}")),
        tenant_id: Some(tenant_id.to_string()),
        ..Default::default()
    }
}

fn error_codes(response: &async_graphql::Response) -> Vec<String> {
    response
        .errors
        .iter()
        .map(|error| {
            error
                .extensions
                .as_ref()
                .and_then(|extensions| extensions.get("code"))
                .map(|code| code.to_string().trim_matches('"').to_string())
                .unwrap_or_default()
        })
        .collect()
}

fn rls_errors(diagnostics: &[SchemaDiagnostic]) -> Vec<&SchemaDiagnostic> {
    diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == SchemaDiagnosticSeverity::Error)
        .collect()
}

fn enforcement_errors(diagnostics: &[SchemaDiagnostic]) -> Vec<&SchemaDiagnostic> {
    rls_errors(diagnostics)
        .into_iter()
        .filter(|diagnostic| {
            diagnostic.message.contains("superuser")
                || diagnostic.message.contains("BYPASSRLS")
                || diagnostic.message.contains("not enforced")
        })
        .collect()
}

#[test]
fn full_target_rls_plan_covers_search_documents_and_the_change_log() {
    let target = graphql_orm_schema_target();
    let plan =
        build_schema_target_rls_plan(DatabaseBackend::Postgres, SchemaPolicy::Managed, &target);
    let joined = plan.statements.join("\n");
    assert!(
        joined.contains(&format!(
            "ALTER TABLE {SEARCH_TABLE} ENABLE ROW LEVEL SECURITY"
        )),
        "{joined}"
    );
    assert!(
        joined.contains(&format!(
            "ALTER TABLE {SEARCH_TABLE} FORCE ROW LEVEL SECURITY"
        )),
        "{joined}"
    );
    assert!(
        plan.statements.iter().any(|statement| {
            statement.starts_with("CREATE POLICY")
                && statement.contains(SEARCH_TABLE)
                && statement.contains("EXISTS (SELECT 1 FROM rls_cov_docs")
        }),
        "{joined}"
    );
    // The trigger is created by a block that resolves the search table once
    // and records its schema and name, never the session's search_path.
    assert!(
        plan.statements.iter().any(|statement| {
            statement.starts_with("DO $graphql_orm$")
                && statement.contains("to_regclass('__graphql_orm_search_rls_cov_docs')")
                && statement.contains("CREATE TRIGGER %I BEFORE DELETE ON %s")
                && statement.contains("'rls_cov_docs'")
        }),
        "{joined}"
    );
    assert!(
        joined.contains("SECURITY INVOKER SET search_path = pg_catalog, pg_temp"),
        "{joined}"
    );
    // Binary keys compare in the ORM's `bin:<hex>` document encoding.
    assert!(
        plan.statements.iter().any(|statement| {
            statement.starts_with("CREATE POLICY")
                && statement.contains("__graphql_orm_search_rls_cov_blobs")
                && statement.contains("decode(substr(")
        }),
        "{joined}"
    );
    assert!(joined.contains(CHANGE_LOG_TABLE), "{joined}");
    // The journal never carries a policy that lets a role subject to RLS write it.
    assert!(!joined.contains("FOR INSERT WITH CHECK (true)"), "{joined}");
    // With the change-journal feature the database records entries through a
    // hardened SECURITY DEFINER trigger on every single-key table.
    let recorder = plan.statements.iter().any(|statement| {
        statement.contains("CREATE OR REPLACE FUNCTION graphql_orm.record_change_journal_entry()")
            && statement.contains("SECURITY DEFINER SET search_path = pg_catalog, pg_temp")
    });
    assert_eq!(recorder, cfg!(feature = "change-journal"), "{joined}");
    let trigger = plan.statements.iter().any(|statement| {
        statement.starts_with("DO $graphql_orm$")
            && statement.contains("CREATE TRIGGER %I AFTER INSERT OR UPDATE OR DELETE ON %s")
            && statement.contains("to_regclass('rls_cov_plain')")
            && statement.contains("INSERT INTO graphql_orm.change_journal_tables")
    });
    assert_eq!(trigger, cfg!(feature = "change-journal"), "{joined}");
    if cfg!(feature = "change-journal") {
        assert!(
            joined.contains(
                "REVOKE ALL ON FUNCTION graphql_orm.record_change_journal_entry() FROM PUBLIC"
            ),
            "{joined}"
        );
        // Every non-owner grant beyond SELECT on the journal, and on the
        // recorder, is revoked.
        assert!(
            joined.contains("REVOKE %s ON %s FROM %s CASCADE"),
            "{joined}"
        );
        assert!(joined.contains("a.privilege_type <> 'SELECT'"), "{joined}");
        assert!(
            joined.contains("REVOKE ALL ON FUNCTION %s FROM %s CASCADE"),
            "{joined}"
        );
    }
    // Non-search RLS entities do not get a search policy.
    assert!(
        !joined.contains("__graphql_orm_search_rls_cov_loose"),
        "{joined}"
    );

    let sqlite =
        build_schema_target_rls_plan(DatabaseBackend::Sqlite, SchemaPolicy::Managed, &target);
    assert!(sqlite.statements.is_empty());
    let external = build_schema_target_rls_plan(
        DatabaseBackend::Postgres,
        SchemaPolicy::ExternalWritable,
        &target,
    );
    assert!(external.statements.is_empty());
}

/// A GraphQL request that reaches an RLS entity without `DbAuthContext` fails
/// closed before any database work, so no pool connection is needed.
#[tokio::test]
async fn missing_db_auth_context_fails_closed_for_rls_entities()
-> Result<(), Box<dyn std::error::Error>> {
    let unreachable = || {
        sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_secs(2))
            .connect_lazy("postgres://nobody:nothing@127.0.0.1:1/none")
    };
    let schema: CoverageSchema = schema_builder(graphql_orm::db::Database::new(unreachable()?))
        .data("test-user".to_string())
        .finish();

    for query in [
        "{ rlsCovDocs { edges { node { id } } } }",
        "{ rlsCovDoc(id: \"00000000-0000-0000-0000-000000000001\") { id } }",
        "{ rlsCovDocsSearch(search: { query: \"alpha\" }) { edges { node { id } } } }",
        "{ rlsCovLooses { edges { node { id } } } }",
        "mutation { createRlsCovDoc(input: { tenantId: \"tenant-a\", title: \"t\" }) { success } }",
    ] {
        let response = schema.execute(Request::new(query)).await;
        assert_eq!(
            error_codes(&response),
            vec!["AUTHORIZATION_MISCONFIGURED".to_string()],
            "{query}: {:?}",
            response.errors
        );
    }

    // A non-RLS root that reaches an RLS entity through a relation filter
    // (also nested in a combinator) or a relation count ordering fails closed.
    for query in [
        "{ rlsCovFolders(where: { files: [{ note: { eq: \"secret\" } }] }) { edges { node { id } } } }",
        "{ rlsCovFolders(where: { and: [{ files: [{ note: { eq: \"secret\" } }] }] }) { edges { node { id } } } }",
        "{ rlsCovFolders(orderBy: [{ FileCount: DESC }]) { edges { node { id } } } }",
    ] {
        let response = schema.execute(Request::new(query)).await;
        assert_eq!(
            error_codes(&response),
            vec!["AUTHORIZATION_MISCONFIGURED".to_string()],
            "{query}: {:?}",
            response.errors
        );
    }
    let unrelated = schema
        .execute(Request::new("{ rlsCovFolders { edges { node { id } } } }"))
        .await;
    assert!(
        !error_codes(&unrelated).contains(&"AUTHORIZATION_MISCONFIGURED".to_string()),
        "{:?}",
        unrelated.errors
    );

    // A non-RLS entity keeps its existing behaviour: the request proceeds to
    // the (unreachable) database instead of failing on the missing context.
    let plain = schema
        .execute(Request::new("{ rlsCovPlains { edges { node { id } } } }"))
        .await;
    assert!(!plain.errors.is_empty());
    assert!(
        !error_codes(&plain).contains(&"AUTHORIZATION_MISCONFIGURED".to_string()),
        "{:?}",
        plain.errors
    );

    // Explicit, named system access is the trusted bypass.
    let system = schema
        .execute(
            Request::new("{ rlsCovDocs { edges { node { id } } } }")
                .data(SystemAccess::new("rls-coverage-test")),
        )
        .await;
    assert!(
        !error_codes(&system).contains(&"AUTHORIZATION_MISCONFIGURED".to_string()),
        "{:?}",
        system.errors
    );

    // A request that carries the context proceeds to the database.
    let with_context = schema
        .execute(Request::new("{ rlsCovDocs { edges { node { id } } } }").data(tenant("tenant-a")))
        .await;
    assert!(
        !error_codes(&with_context).contains(&"AUTHORIZATION_MISCONFIGURED".to_string()),
        "{:?}",
        with_context.errors
    );

    // LegacyPermissive keeps the pre-fix behaviour.
    let legacy: CoverageSchema = schema_builder(
        graphql_orm::db::Database::builder(unreachable()?)
            .authorization_mode(AuthorizationMode::LegacyPermissive)
            .build(),
    )
    .data("test-user".to_string())
    .finish();
    let legacy = legacy
        .execute(Request::new("{ rlsCovDocs { edges { node { id } } } }"))
        .await;
    assert!(
        !error_codes(&legacy).contains(&"AUTHORIZATION_MISCONFIGURED".to_string()),
        "{:?}",
        legacy.errors
    );
    Ok(())
}

struct Provisioned {
    owner: sqlx::PgPool,
}

async fn provision() -> Result<Provisioned, Box<dyn std::error::Error>> {
    let owner = owned_postgres::OwnedPostgres::pool("postgres-rls-coverage", 5).await?;
    let database = graphql_orm::db::Database::new(owner.clone());
    let target = graphql_orm_schema_target();
    let plan = database
        .schema()
        .plan_schema_target("1", "rls coverage", &target)
        .await?;
    database
        .schema()
        .apply_schema_target(&plan, ApplyOptions::default())
        .await?;
    Ok(Provisioned { owner })
}

/// Create a login role with ordinary runtime grants and connect a pool as it.
async fn role_pool(
    owner: &sqlx::PgPool,
    label: &str,
    attributes: &str,
) -> Result<(String, sqlx::PgPool), Box<dyn std::error::Error>> {
    let token = graphql_orm::uuid::Uuid::new_v4().simple().to_string();
    let role = format!("rls_{label}_{token}");
    let password = format!("synthetic_{token}");
    sqlx::query(&format!(
        "CREATE ROLE {role} LOGIN {attributes} PASSWORD '{password}'"
    ))
    .execute(owner)
    .await?;
    // The helper functions keep their default PUBLIC EXECUTE. A runtime role
    // needs no privilege on the change journal beyond reading it under RLS,
    // and no EXECUTE on its recorder.
    for grant in [
        format!("GRANT USAGE ON SCHEMA public, graphql_orm TO {role}"),
        format!("GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO {role}"),
        format!(
            "DO $$ BEGIN IF to_regclass('{CHANGE_LOG_TABLE}') IS NOT NULL THEN REVOKE ALL ON {CHANGE_LOG_TABLE} FROM {role}; GRANT SELECT ON {CHANGE_LOG_TABLE} TO {role}; END IF; END $$"
        ),
    ] {
        sqlx::query(&grant).execute(owner).await?;
    }
    let options = owner
        .connect_options()
        .as_ref()
        .clone()
        .username(&role)
        .password(&password);
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .connect_with(options)
        .await?;
    Ok((role, pool))
}

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn validate_target_reports_roles_that_bypass_rls() -> Result<(), Box<dyn std::error::Error>> {
    let Provisioned { owner } = provision().await?;
    let target = graphql_orm_schema_target();

    // The container owner is a superuser.
    let superuser = graphql_orm::db::Database::new(owner.clone())
        .schema()
        .validate_target(&target)
        .await?;
    let errors = enforcement_errors(&superuser.rls_diagnostics);
    assert!(
        errors
            .iter()
            .any(|diagnostic| diagnostic.message.contains("superuser")),
        "{:#?}",
        superuser.rls_diagnostics
    );

    let (_, bypass) = role_pool(&owner, "bypass", "NOSUPERUSER BYPASSRLS").await?;
    let bypass_database = graphql_orm::db::Database::new(bypass);
    let bypass_report = bypass_database.schema().validate_target(&target).await?;
    assert!(
        enforcement_errors(&bypass_report.rls_diagnostics)
            .iter()
            .any(|diagnostic| diagnostic.message.contains("BYPASSRLS")),
        "{:#?}",
        bypass_report.rls_diagnostics
    );
    // The standalone startup check reports the same refusal.
    let startup = bypass_database
        .schema()
        .validate_rls_enforcement(&target)
        .await?;
    assert!(
        startup.iter().any(
            |diagnostic| diagnostic.severity == SchemaDiagnosticSeverity::Error
                && diagnostic.message.contains("BYPASSRLS")
        ),
        "{startup:#?}"
    );

    // Owning a table declared `force = false` exempts the owner from its policies.
    let (loose_role, loose) = role_pool(&owner, "loose_owner", "NOSUPERUSER NOBYPASSRLS").await?;
    sqlx::query(&format!("ALTER TABLE rls_cov_loose OWNER TO {loose_role}"))
        .execute(&owner)
        .await?;
    let loose_report = graphql_orm::db::Database::new(loose)
        .schema()
        .validate_target(&target)
        .await?;
    let loose_errors = enforcement_errors(&loose_report.rls_diagnostics);
    assert!(
        loose_errors
            .iter()
            .any(|diagnostic| diagnostic.table.as_deref() == Some("rls_cov_loose")),
        "{:#?}",
        loose_report.rls_diagnostics
    );
    assert!(
        !loose_errors
            .iter()
            .any(|diagnostic| diagnostic.table.as_deref() == Some("rls_cov_docs")),
        "{:#?}",
        loose_report.rls_diagnostics
    );

    // A restricted runtime role is enforced everywhere.
    let (_, runtime) = role_pool(&owner, "runtime", "NOSUPERUSER NOBYPASSRLS").await?;
    let runtime_report = graphql_orm::db::Database::new(runtime)
        .schema()
        .validate_target(&target)
        .await?;
    assert!(
        enforcement_errors(&runtime_report.rls_diagnostics).is_empty(),
        "{:#?}",
        runtime_report.rls_diagnostics
    );
    // Search-table drift validation is clean once the plan is applied.
    assert!(
        !rls_errors(&runtime_report.rls_diagnostics)
            .iter()
            .any(|diagnostic| diagnostic.table.as_deref() == Some(SEARCH_TABLE)),
        "{:#?}",
        runtime_report.rls_diagnostics
    );

    // Removing search-table RLS is reported as drift.
    sqlx::query(&format!(
        "ALTER TABLE {SEARCH_TABLE} NO FORCE ROW LEVEL SECURITY"
    ))
    .execute(&owner)
    .await?;
    let drifted = graphql_orm::db::Database::new(owner.clone())
        .schema()
        .validate_target(&target)
        .await?;
    assert!(
        rls_errors(&drifted.rls_diagnostics)
            .iter()
            .any(
                |diagnostic| diagnostic.table.as_deref() == Some(SEARCH_TABLE)
                    && diagnostic.message.contains("not forced")
            ),
        "{:#?}",
        drifted.rls_diagnostics
    );
    Ok(())
}

async fn count_as(
    pool: &sqlx::PgPool,
    sql: &str,
    auth: Option<&DbAuthContext>,
) -> Result<i64, Box<dyn std::error::Error>> {
    let rows = fetch_rows_with_auth::<PostgresBackend>(pool, sql, &[], auth).await?;
    Ok(rows[0].try_get::<i64, _>("count")?)
}

async fn execute_as(
    pool: &sqlx::PgPool,
    sql: &str,
    auth: &DbAuthContext,
) -> Result<u64, sqlx::Error> {
    let mut tx = pool.begin().await?;
    apply_db_auth_context_to_transaction::<PostgresBackend>(&mut tx, Some(auth)).await?;
    let result = sqlx::query(sql).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(result.rows_affected())
}

async fn create_doc(
    schema: &CoverageSchema,
    tenant_id: &str,
    title: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let response = schema
        .execute(
            Request::new(format!(
                "mutation {{ createRlsCovDoc(input: {{ tenantId: \"{tenant_id}\", title: \"{title}\", body: \"private body\" }}) {{ success rlsCovDoc {{ id }} }} }}"
            ))
            .data(tenant(tenant_id)),
        )
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let json = response.data.into_json()?;
    Ok(json["createRlsCovDoc"]["rlsCovDoc"]["id"]
        .as_str()
        .expect("created id")
        .to_string())
}

#[cfg(feature = "change-journal")]
async fn search_hits(
    schema: &CoverageSchema,
    tenant_id: &str,
    query: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let response = schema
        .execute(
            Request::new(format!(
                "{{ rlsCovDocsSearch(search: {{ query: \"{query}\" }}) {{ edges {{ node {{ id }} }} }} }}"
            ))
            .data(tenant(tenant_id)),
        )
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let json = response.data.into_json()?;
    Ok(json["rlsCovDocsSearch"]["edges"]
        .as_array()
        .expect("search edges")
        .iter()
        .map(|edge| edge["node"]["id"].as_str().expect("id").to_string())
        .collect())
}

#[cfg(feature = "change-journal")]
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn runtime_role_cannot_reach_other_tenant_search_documents_or_change_log()
-> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::graphql::orm::ChangeWindow;

    let Provisioned { owner } = provision().await?;
    let owner_database = graphql_orm::db::Database::new(owner.clone());
    owner_database.ensure_change_journal_table().await?;
    // Re-applying the target secures a change log created before the plan.
    let target = graphql_orm_schema_target();
    let plan = owner_database
        .schema()
        .plan_schema_target("2", "rls coverage change log", &target)
        .await?;
    owner_database
        .schema()
        .apply_schema_target(&plan, ApplyOptions::default())
        .await?;

    let (_, runtime) = role_pool(&owner, "runtime", "NOSUPERUSER NOBYPASSRLS").await?;
    let runtime_database = graphql_orm::db::Database::builder(runtime.clone())
        .change_journal_enabled(true)
        .build();
    let schema: CoverageSchema = schema_builder(runtime_database.clone())
        .data("test-user".to_string())
        .finish();

    let alpha = create_doc(&schema, "tenant-a", "Quarterly alpha report").await?;
    let beta = create_doc(&schema, "tenant-b", "Beta planning notes").await?;

    // Raw SQL as the runtime role sees only the tenant's own search documents.
    let search_count = format!("SELECT COUNT(*)::BIGINT AS count FROM {SEARCH_TABLE}");
    assert_eq!(
        count_as(&runtime, &search_count, Some(&tenant("tenant-a"))).await?,
        1
    );
    assert_eq!(
        count_as(&runtime, &search_count, Some(&tenant("tenant-b"))).await?,
        1
    );
    assert_eq!(count_as(&runtime, &search_count, None).await?, 0);
    let alpha_text = format!(
        "SELECT COUNT(*)::BIGINT AS count FROM {SEARCH_TABLE} WHERE entity_pk = '{alpha}' AND document_text LIKE '%alpha%'"
    );
    assert_eq!(
        count_as(&runtime, &alpha_text, Some(&tenant("tenant-b"))).await?,
        0
    );
    assert_eq!(
        count_as(&runtime, &alpha_text, Some(&tenant("tenant-a"))).await?,
        1
    );

    // Another tenant cannot alter, delete, or forge search documents.
    let tenant_b = tenant("tenant-b");
    assert_eq!(
        execute_as(
            &runtime,
            &format!(
                "UPDATE {SEARCH_TABLE} SET document_text = 'poisoned' WHERE entity_pk = '{alpha}'"
            ),
            &tenant_b,
        )
        .await?,
        0
    );
    assert_eq!(
        execute_as(
            &runtime,
            &format!("DELETE FROM {SEARCH_TABLE} WHERE entity_pk = '{alpha}'"),
            &tenant_b,
        )
        .await?,
        0
    );
    let forged = graphql_orm::uuid::Uuid::new_v4();
    let direct_insert = execute_as(
        &runtime,
        &format!(
            "INSERT INTO {SEARCH_TABLE} (entity_pk, entity_pk_json, document_text, document_vector, updated_at) \
             VALUES ('{forged}', '\"{forged}\"'::jsonb, 'alpha', to_tsvector('alpha'), 0)"
        ),
        &tenant_b,
    )
    .await;
    assert!(direct_insert.is_err(), "{direct_insert:?}");

    // The runtime role can neither read nor write the change log.
    let journal_count = format!("SELECT COUNT(*)::BIGINT AS count FROM {CHANGE_LOG_TABLE}");
    assert_eq!(
        count_as(&runtime, &journal_count, Some(&tenant("tenant-a"))).await?,
        0
    );
    assert_eq!(
        count_as(&runtime, &journal_count, Some(&tenant("tenant-b"))).await?,
        0
    );
    assert_eq!(count_as(&runtime, &journal_count, None).await?, 0);
    for statement in [
        format!("DELETE FROM {CHANGE_LOG_TABLE}"),
        format!("UPDATE {CHANGE_LOG_TABLE} SET action = 'delete'"),
    ] {
        let changed = execute_as(&runtime, &statement, &tenant_b).await;
        assert!(
            !matches!(changed, Ok(rows) if rows > 0),
            "{statement}: {changed:?}"
        );
    }
    assert!(count_as(&owner, &journal_count, None).await? >= 2);

    // The ORM's own search is unchanged for the authorized tenant.
    assert_eq!(
        search_hits(&schema, "tenant-a", "alpha").await?,
        vec![alpha.clone()]
    );
    assert!(search_hits(&schema, "tenant-b", "alpha").await?.is_empty());
    assert_eq!(
        search_hits(&schema, "tenant-b", "planning").await?,
        vec![beta.clone()]
    );

    // Updates keep the document in sync under the new policy.
    let updated = schema
        .execute(
            Request::new(format!(
                "mutation {{ updateRlsCovDoc(id: \"{alpha}\", input: {{ title: \"Quarterly gamma report\" }}) {{ success }} }}"
            ))
            .data(tenant("tenant-a")),
        )
        .await;
    assert!(updated.errors.is_empty(), "{:?}", updated.errors);
    assert_eq!(
        search_hits(&schema, "tenant-a", "gamma").await?,
        vec![alpha.clone()]
    );
    assert!(search_hits(&schema, "tenant-a", "alpha").await?.is_empty());

    // Incremental export needs a privileged role: the owner reads every entry and the
    // runtime role is refused instead of silently exporting nothing.
    let window = ChangeWindow {
        after_snapshot_id: None,
        until: i64::MAX,
    };
    let exported = owner_database.export_changes(window.clone()).await?;
    assert!(exported.len() >= 3, "{exported:?}");
    let refused = runtime_database.export_changes(window).await;
    assert!(refused.is_err(), "{refused:?}");

    // Deleting the base row removes its search document even though the
    // document is only visible while the base row exists.
    let deleted = schema
        .execute(
            Request::new(format!(
                "mutation {{ deleteRlsCovDoc(id: \"{alpha}\") {{ success }} }}"
            ))
            .data(tenant("tenant-a")),
        )
        .await;
    assert!(deleted.errors.is_empty(), "{:?}", deleted.errors);
    let orphaned =
        format!("SELECT COUNT(*)::BIGINT AS count FROM {SEARCH_TABLE} WHERE entity_pk = '{alpha}'");
    assert_eq!(count_as(&owner, &orphaned, None).await?, 0);
    assert!(search_hits(&schema, "tenant-a", "gamma").await?.is_empty());

    // The validated runtime role reports no RLS drift on the protected tables.
    let report = runtime_database.schema().validate_target(&target).await?;
    let protected = rls_errors(&report.rls_diagnostics)
        .into_iter()
        .filter(|diagnostic| {
            matches!(
                diagnostic.table.as_deref(),
                Some(SEARCH_TABLE) | Some(CHANGE_LOG_TABLE)
            )
        })
        .collect::<Vec<_>>();
    assert!(protected.is_empty(), "{:#?}", report.rls_diagnostics);
    assert!(
        enforcement_errors(&report.rls_diagnostics).is_empty(),
        "{:#?}",
        report.rls_diagnostics
    );

    // An extra read policy on the change log is reported as drift.
    sqlx::query(&format!(
        "CREATE POLICY graphql_orm_extra_read ON {CHANGE_LOG_TABLE} FOR SELECT USING (true)"
    ))
    .execute(&owner)
    .await?;
    let drifted = owner_database.schema().validate_target(&target).await?;
    assert!(
        rls_errors(&drifted.rls_diagnostics)
            .iter()
            .any(|diagnostic| diagnostic.table.as_deref() == Some(CHANGE_LOG_TABLE)),
        "{:#?}",
        drifted.rls_diagnostics
    );
    Ok(())
}

/// Run statements in one transaction as `pool`'s role with `auth` applied.
async fn script_as(
    pool: &sqlx::PgPool,
    statements: &[&str],
    auth: Option<&DbAuthContext>,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    apply_db_auth_context_to_transaction::<PostgresBackend>(&mut tx, auth).await?;
    for statement in statements {
        sqlx::raw_sql(statement).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn reapply_target(
    owner: &sqlx::PgPool,
    version: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let database = graphql_orm::db::Database::new(owner.clone());
    let target = graphql_orm_schema_target();
    let plan = database
        .schema()
        .plan_schema_target(version, "reapply rls coverage", &target)
        .await?;
    database
        .schema()
        .apply_schema_target(&plan, ApplyOptions::default())
        .await?;
    Ok(())
}

async fn errors_on(
    owner: &sqlx::PgPool,
    table: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let report = graphql_orm::db::Database::new(owner.clone())
        .schema()
        .validate_target(&graphql_orm_schema_target())
        .await?;
    Ok(rls_errors(&report.rls_diagnostics)
        .into_iter()
        .filter(|diagnostic| diagnostic.table.as_deref() == Some(table))
        .map(|diagnostic| diagnostic.message.clone())
        .collect())
}

fn coverage_schema(pool: sqlx::PgPool) -> CoverageSchema {
    let registry = StaticDependentDeletes::<PostgresBackend>::new(
        RuntimeDependentLimits::new(10, 4).expect("limits"),
    )
    .register::<RlsCovFolder>()
    .expect("register folder")
    .register::<RlsCovFile>()
    .expect("register file");
    schema_builder(graphql_orm::db::Database::new(pool))
        .data("test-user".to_string())
        .data(registry)
        .finish()
}

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn search_delete_trigger_resists_redirection_and_drift()
-> Result<(), Box<dyn std::error::Error>> {
    let Provisioned { owner } = provision().await?;
    let (_, runtime) = role_pool(&owner, "runtime", "NOSUPERUSER NOBYPASSRLS").await?;
    let schema = coverage_schema(runtime.clone());
    let tenant_a = tenant("tenant-a");
    let document = |id: &str| {
        format!("SELECT COUNT(*)::BIGINT AS count FROM {SEARCH_TABLE} WHERE entity_pk = '{id}'")
    };

    // A session search_path cannot hide the search table from the trigger.
    let pathless = create_doc(&schema, "tenant-a", "Pathless secret").await?;
    script_as(
        &runtime,
        &[
            "SET LOCAL search_path = pg_catalog",
            &format!("DELETE FROM public.rls_cov_docs WHERE id = '{pathless}'"),
        ],
        Some(&tenant_a),
    )
    .await?;
    assert_eq!(count_as(&owner, &document(&pathless), None).await?, 0);
    // ...not even for a privileged session.
    let privileged = create_doc(&schema, "tenant-a", "Privileged secret").await?;
    script_as(
        &owner,
        &[
            "SET LOCAL search_path = pg_catalog",
            &format!("DELETE FROM public.rls_cov_docs WHERE id = '{privileged}'"),
        ],
        None,
    )
    .await?;
    assert_eq!(count_as(&owner, &document(&privileged), None).await?, 0);

    // A same-named pg_temp table cannot capture the document delete.
    let shadowed = create_doc(&schema, "tenant-a", "Shadowed secret").await?;
    script_as(
        &runtime,
        &[
            // ON COMMIT DROP keeps the pooled session clean for later writes.
            &format!("CREATE TEMP TABLE {SEARCH_TABLE} (entity_pk text) ON COMMIT DROP"),
            &format!("DELETE FROM rls_cov_docs WHERE id = '{shadowed}'"),
        ],
        Some(&tenant_a),
    )
    .await?;
    assert_eq!(count_as(&owner, &document(&shadowed), None).await?, 0);

    // A same-named pg_temp type cannot change how the trigger reads the key:
    // a `text` domain over `name` would truncate a key longer than 63 bytes,
    // so the document delete would match nothing.
    let long_key = format!("file-{}", "k".repeat(75));
    for statement in [
        "mutation { createRlsCovFolder(input: { id: \"fshadow\", label: \"Shadow folder\" }) { success } }".to_string(),
        format!("mutation {{ createRlsCovFile(input: {{ id: \"{long_key}\", tenantId: \"tenant-a\", folderId: \"fshadow\", note: \"long key secret\" }}) {{ success }} }}"),
    ] {
        let response = schema
            .execute(Request::new(statement.as_str()).data(tenant_a.clone()))
            .await;
        assert!(response.errors.is_empty(), "{statement}: {:?}", response.errors);
    }
    let long_document = format!(
        "SELECT COUNT(*)::BIGINT AS count FROM __graphql_orm_search_rls_cov_files WHERE entity_pk = '{long_key}'"
    );
    assert_eq!(count_as(&owner, &long_document, None).await?, 1);
    script_as(
        &runtime,
        &[
            "CREATE DOMAIN pg_temp.text AS pg_catalog.name",
            &format!("DELETE FROM rls_cov_files WHERE id = '{long_key}'"),
            // Temp types outlive the transaction; drop it so the pooled
            // session's own later ORM writes are unaffected.
            "DROP DOMAIN pg_temp.text",
        ],
        Some(&tenant_a),
    )
    .await?;
    assert_eq!(count_as(&owner, &long_document, None).await?, 0);

    // A missing search table refuses the base delete instead of orphaning.
    let guarded = create_doc(&schema, "tenant-a", "Guarded secret").await?;
    sqlx::query(&format!(
        "ALTER TABLE {SEARCH_TABLE} RENAME TO moved_search"
    ))
    .execute(&owner)
    .await?;
    let refused = execute_as(
        &runtime,
        &format!("DELETE FROM rls_cov_docs WHERE id = '{guarded}'"),
        &tenant_a,
    )
    .await;
    assert!(refused.is_err(), "{refused:?}");
    sqlx::query(&format!(
        "ALTER TABLE moved_search RENAME TO {SEARCH_TABLE}"
    ))
    .execute(&owner)
    .await?;
    assert_eq!(count_as(&owner, &document(&guarded), None).await?, 1);

    // Drift: a trigger that does not fire in ordinary sessions...
    assert!(errors_on(&owner, SEARCH_TABLE).await?.is_empty());
    sqlx::query("ALTER TABLE rls_cov_docs ENABLE REPLICA TRIGGER graphql_orm_search_delete")
        .execute(&owner)
        .await?;
    let replica = errors_on(&owner, SEARCH_TABLE).await?;
    assert!(
        replica
            .iter()
            .any(|message| message.contains("does not fire")),
        "{replica:?}"
    );
    sqlx::query("ALTER TABLE rls_cov_docs ENABLE TRIGGER graphql_orm_search_delete")
        .execute(&owner)
        .await?;
    // ...a trigger pointed at another entity's search table...
    sqlx::query("DROP TRIGGER graphql_orm_search_delete ON rls_cov_files")
        .execute(&owner)
        .await?;
    sqlx::query(&format!(
        "CREATE TRIGGER graphql_orm_search_delete BEFORE DELETE ON rls_cov_files FOR EACH ROW EXECUTE FUNCTION graphql_orm.delete_search_document('public', '{SEARCH_TABLE}', 'id', 'text')"
    ))
    .execute(&owner)
    .await?;
    let wrong = errors_on(&owner, "__graphql_orm_search_rls_cov_files").await?;
    assert!(
        wrong
            .iter()
            .any(|message| message.contains("does not target")),
        "{wrong:?}"
    );
    // ...and a modified trigger function are all reported.
    sqlx::query("CREATE OR REPLACE FUNCTION graphql_orm.delete_search_document() RETURNS trigger LANGUAGE plpgsql SET search_path = pg_catalog AS $$ BEGIN RETURN OLD; END $$")
        .execute(&owner)
        .await?;
    let modified = errors_on(&owner, SEARCH_TABLE).await?;
    assert!(
        modified.iter().any(|message| message.contains("differs")),
        "{modified:?}"
    );
    reapply_target(&owner, "2").await?;
    for table in [
        SEARCH_TABLE,
        "__graphql_orm_search_rls_cov_files",
        "__graphql_orm_search_rls_cov_blobs",
    ] {
        let remaining = errors_on(&owner, table).await?;
        assert!(remaining.is_empty(), "{table}: {remaining:?}");
    }

    // Binary keys: the policy and trigger use the ORM's `bin:<hex>` key.
    let digest = vec![0xab_u8, 0x01, 0xff];
    let runtime_database = graphql_orm::db::Database::new(runtime.clone());
    let mut context =
        graphql_orm::graphql::orm::MutationContext::<PostgresBackend>::begin_with_auth(
            &runtime_database,
            graphql_orm::graphql::orm::TransactionMode::Default,
            Some(&tenant_a),
        )
        .await?;
    context
        .insert::<RlsCovBlob>(CreateRlsCovBlobInput {
            digest: digest.clone(),
            tenant_id: "tenant-a".to_string(),
            body: "binary secret".to_string(),
        })
        .await?;
    context.commit_and_emit().await?;
    let blob_document = "SELECT COUNT(*)::BIGINT AS count FROM __graphql_orm_search_rls_cov_blobs WHERE entity_pk = 'bin:ab01ff'";
    assert_eq!(count_as(&owner, blob_document, None).await?, 1);
    assert_eq!(count_as(&runtime, blob_document, Some(&tenant_a)).await?, 1);
    assert_eq!(
        count_as(&runtime, blob_document, Some(&tenant("tenant-b"))).await?,
        0
    );
    assert_eq!(
        execute_as(
            &runtime,
            "DELETE FROM rls_cov_blobs WHERE digest = '\\xab01ff'::bytea",
            &tenant_a
        )
        .await?,
        1
    );
    assert_eq!(count_as(&owner, blob_document, None).await?, 0);

    // Dependent deletes from a non-RLS parent into RLS children.
    let (_, bypass) = role_pool(&owner, "delete_bypass", "NOSUPERUSER BYPASSRLS").await?;
    let delete_schema = coverage_schema(bypass);
    for (statement, auth) in [
        (
            "mutation { createRlsCovFolder(input: { id: \"f1\", label: \"Folder\" }) { success } }",
            &tenant_a,
        ),
        (
            "mutation { createRlsCovFile(input: { id: \"c1\", tenantId: \"tenant-a\", folderId: \"f1\", note: \"cascade secret\" }) { success } }",
            &tenant_a,
        ),
        (
            "mutation { createRlsCovFolder(input: { id: \"f2\", label: \"Folder 2\" }) { success } }",
            &tenant_a,
        ),
        (
            "mutation { createRlsCovFile(input: { id: \"c2\", tenantId: \"tenant-a\", folderId: \"f2\", note: \"replica secret\" }) { success } }",
            &tenant_a,
        ),
    ] {
        let response = schema
            .execute(Request::new(statement).data(auth.clone()))
            .await;
        assert!(
            response.errors.is_empty(),
            "{statement}: {:?}",
            response.errors
        );
    }
    // Without request context the dependent delete stops at the RLS child.
    let without_context = delete_schema
        .execute(Request::new(
            "mutation { deleteRlsCovFolder(id: \"f1\") { success } }",
        ))
        .await;
    assert_eq!(
        error_codes(&without_context),
        vec!["AUTHORIZATION_MISCONFIGURED".to_string()],
        "{:?}",
        without_context.errors
    );
    // With context, the privileged delete pool cascades and the allow-listed
    // trigger removes the child's search document.
    let deleted = delete_schema
        .execute(
            Request::new("mutation { deleteRlsCovFolder(id: \"f1\") { success } }")
                .data(tenant_a.clone()),
        )
        .await;
    assert!(deleted.errors.is_empty(), "{:?}", deleted.errors);
    let file_document = |id: &str| {
        format!(
            "SELECT COUNT(*)::BIGINT AS count FROM __graphql_orm_search_rls_cov_files WHERE entity_pk = '{id}'"
        )
    };
    assert_eq!(
        count_as(
            &owner,
            "SELECT COUNT(*)::BIGINT AS count FROM rls_cov_files WHERE id = 'c1'",
            None
        )
        .await?,
        0
    );
    assert_eq!(count_as(&owner, &file_document("c1"), None).await?, 0);
    // A trigger that does not fire normally is not allow-listed.
    sqlx::query("ALTER TABLE rls_cov_files ENABLE REPLICA TRIGGER graphql_orm_search_delete")
        .execute(&owner)
        .await?;
    let refused = delete_schema
        .execute(
            Request::new("mutation { deleteRlsCovFolder(id: \"f2\") { success } }")
                .data(tenant_a.clone()),
        )
        .await;
    assert!(
        refused
            .errors
            .iter()
            .any(|error| error.message.contains("Dependent delete refused")),
        "{:?}",
        refused.errors
    );
    sqlx::query("ALTER TABLE rls_cov_files ENABLE TRIGGER graphql_orm_search_delete")
        .execute(&owner)
        .await?;

    // Dropping the search index drops its delete trigger.
    let database = graphql_orm::db::Database::new(owner.clone());
    let current = graphql_orm_schema_target().schema;
    let mut without_search = current.clone();
    for table in &mut without_search.tables {
        if table.table_name == "rls_cov_docs" {
            table.search_indexes.clear();
        }
    }
    let plan = database.schema().plan_migration(
        "3",
        "drop rls_cov_docs search",
        &current,
        &without_search,
    )?;
    assert!(
        plan.statements.iter().any(|statement| statement
            .contains("DROP TRIGGER IF EXISTS \"graphql_orm_search_delete\" ON rls_cov_docs")),
        "{:#?}",
        plan.statements
    );
    database
        .schema()
        .apply_migration(
            &plan,
            ApplyOptions {
                allow_destructive: true,
                require_clean_schema: false,
                ..ApplyOptions::default()
            },
        )
        .await?;
    assert_eq!(
        count_as(
            &owner,
            "SELECT COUNT(*)::BIGINT AS count FROM pg_trigger WHERE tgrelid = 'rls_cov_docs'::regclass AND tgname = 'graphql_orm_search_delete'",
            None
        )
        .await?,
        0
    );
    assert_eq!(
        execute_as(
            &runtime,
            &format!("DELETE FROM rls_cov_docs WHERE id = '{guarded}'"),
            &tenant_a
        )
        .await?,
        1
    );
    Ok(())
}

/// Journal entries for `key`, as `(entity, table, action)`, read by the owner.
#[cfg(feature = "change-journal")]
async fn journal_entries(
    owner: &sqlx::PgPool,
    key: &str,
) -> Result<Vec<(String, String, String)>, Box<dyn std::error::Error>> {
    let rows = sqlx::query(&format!(
        "SELECT entity_name, table_name, action FROM {CHANGE_LOG_TABLE} WHERE primary_key = $1 ORDER BY action"
    ))
    .bind(key)
    .fetch_all(owner)
    .await?;
    Ok(rows
        .iter()
        .map(|row| {
            (
                row.get::<String, _>("entity_name"),
                row.get::<String, _>("table_name"),
                row.get::<String, _>("action"),
            )
        })
        .collect())
}

/// Validation errors about the change journal or its recorder on `table`.
#[cfg(feature = "change-journal")]
async fn journal_errors_on(
    owner: &sqlx::PgPool,
    table: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    Ok(errors_on(owner, table)
        .await?
        .into_iter()
        .filter(|message| {
            table == CHANGE_LOG_TABLE
                || message.to_ascii_lowercase().contains("change journal")
                || message.contains("record_change_journal_entry")
        })
        // Validating with the owner pool always reports the owner itself;
        // `journal_privileges_beyond_select_are_refused` asserts that.
        .filter(|message| !message.contains("acts as the journal owner"))
        .collect())
}

/// The database records change journal entries; the runtime role cannot.
///
/// Every entry must correspond to a row change made in the recording
/// transaction. A runtime-role session cannot add an entry for another
/// tenant's row, while the ORM's own writes still journal.
#[cfg(feature = "change-journal")]
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn runtime_role_cannot_write_change_journal_entries() -> Result<(), Box<dyn std::error::Error>>
{
    let Provisioned { owner } = provision().await?;
    graphql_orm::db::Database::new(owner.clone())
        .ensure_change_journal_table()
        .await?;
    let (_, runtime) = role_pool(&owner, "journal", "NOSUPERUSER NOBYPASSRLS").await?;
    let runtime_database = graphql_orm::db::Database::builder(runtime.clone())
        .change_journal_enabled(true)
        .build();
    let schema: CoverageSchema = schema_builder(runtime_database.clone())
        .data("test-user".to_string())
        .finish();
    let tenant_a = tenant("tenant-a");
    let tenant_b = tenant("tenant-b");
    let tenant_a_doc = create_doc(&schema, "tenant-a", "Tenant A ledger").await?;
    let doc_entry = |action: &str| {
        (
            "RlsCovDoc".to_string(),
            "rls_cov_docs".to_string(),
            action.to_string(),
        )
    };
    assert_eq!(
        journal_entries(&owner, &tenant_a_doc).await?,
        vec![doc_entry("create")]
    );

    // A direct insert is refused, with or without the recording setting.
    let direct_insert = format!(
        "INSERT INTO {CHANGE_LOG_TABLE} (id, entity_name, table_name, primary_key, action, changed_at) \
         VALUES (gen_random_uuid(), 'RlsCovDoc', 'rls_cov_docs', '{tenant_a_doc}', 'delete', 1)"
    );
    let direct = execute_as(&runtime, &direct_insert, &tenant_b).await;
    assert!(direct.is_err(), "{direct:?}");
    let flagged = script_as(
        &runtime,
        &[
            "SELECT set_config('graphql_orm.change_journal', 'on', true)",
            &direct_insert,
        ],
        Some(&tenant_b),
    )
    .await;
    assert!(flagged.is_err(), "{flagged:?}");

    // Another tenant's row is invisible, so recording a raw delete of it
    // changes nothing and journals nothing.
    script_as(
        &runtime,
        &[
            "SELECT set_config('graphql_orm.change_journal', 'on', true)",
            &format!("DELETE FROM rls_cov_docs WHERE id = '{tenant_a_doc}'"),
        ],
        Some(&tenant_b),
    )
    .await?;
    assert_eq!(
        journal_entries(&owner, &tenant_a_doc).await?,
        vec![doc_entry("create")]
    );
    let tenant_a_rows =
        format!("SELECT COUNT(*)::BIGINT AS count FROM rls_cov_docs WHERE id = '{tenant_a_doc}'");
    assert_eq!(count_as(&owner, &tenant_a_rows, None).await?, 1);

    // Genuine ORM writes journal with the entity, table, key and action.
    let updated = schema
        .execute(
            Request::new(format!(
                "mutation {{ updateRlsCovDoc(id: \"{tenant_a_doc}\", input: {{ title: \"Tenant A revised ledger\" }}) {{ success }} }}"
            ))
            .data(tenant_a.clone()),
        )
        .await;
    assert!(updated.errors.is_empty(), "{:?}", updated.errors);
    let deleted = schema
        .execute(
            Request::new(format!(
                "mutation {{ deleteRlsCovDoc(id: \"{tenant_a_doc}\") {{ success }} }}"
            ))
            .data(tenant_a.clone()),
        )
        .await;
    assert!(deleted.errors.is_empty(), "{:?}", deleted.errors);
    assert_eq!(
        journal_entries(&owner, &tenant_a_doc).await?,
        vec![
            doc_entry("create"),
            doc_entry("delete"),
            doc_entry("update")
        ]
    );

    // Binary keys journal in the ORM's `bin:<hex>` form.
    let mut context =
        graphql_orm::graphql::orm::MutationContext::<PostgresBackend>::begin_with_auth(
            &runtime_database,
            graphql_orm::graphql::orm::TransactionMode::Default,
            Some(&tenant_a),
        )
        .await?;
    context
        .insert::<RlsCovBlob>(CreateRlsCovBlobInput {
            digest: vec![0xab, 0x01, 0xff],
            tenant_id: "tenant-a".to_string(),
            body: "binary journal".to_string(),
        })
        .await?;
    context.commit_and_emit().await?;
    assert_eq!(
        journal_entries(&owner, "bin:ab01ff").await?,
        vec![(
            "RlsCovBlob".to_string(),
            "rls_cov_blobs".to_string(),
            "create".to_string()
        )]
    );

    // Writes outside an ORM journal transaction, such as restore imports,
    // are not recorded.
    script_as(
        &owner,
        &["INSERT INTO rls_cov_plain (id, label) VALUES ('plain-raw', 'raw')"],
        None,
    )
    .await?;
    assert!(journal_entries(&owner, "plain-raw").await?.is_empty());

    // No validation drift on the recorder or the journal.
    for table in [CHANGE_LOG_TABLE, "rls_cov_docs", "rls_cov_plain"] {
        let errors = journal_errors_on(&owner, table).await?;
        assert!(errors.is_empty(), "{table}: {errors:?}");
    }

    // A missing recorder trigger fails closed: the write rolls back.
    sqlx::query("DROP TRIGGER graphql_orm_change_journal ON rls_cov_plain")
        .execute(&owner)
        .await?;
    let create_plain = "mutation { createRlsCovPlain(input: { label: \"journaled plain\" }) { success rlsCovPlain { id } } }";
    let refused = schema
        .execute(Request::new(create_plain).data(tenant_a.clone()))
        .await;
    assert!(!refused.errors.is_empty(), "{:?}", refused.data);
    let plain_rows =
        "SELECT COUNT(*)::BIGINT AS count FROM rls_cov_plain WHERE label = 'journaled plain'";
    assert_eq!(count_as(&owner, plain_rows, None).await?, 0);
    let missing = journal_errors_on(&owner, "rls_cov_plain").await?;
    assert!(
        missing
            .iter()
            .any(|message| message.contains("Missing change journal trigger")),
        "{missing:?}"
    );

    // Drift: a trigger that does not fire in ordinary sessions, a modified
    // recorder, and the INSERT policy of earlier releases are all reported.
    sqlx::query("ALTER TABLE rls_cov_docs ENABLE REPLICA TRIGGER graphql_orm_change_journal")
        .execute(&owner)
        .await?;
    let replica = journal_errors_on(&owner, "rls_cov_docs").await?;
    assert!(
        replica
            .iter()
            .any(|message| message.contains("does not fire")),
        "{replica:?}"
    );
    sqlx::query(&format!(
        "CREATE POLICY graphql_orm_graphql_orm_change_log_insert ON {CHANGE_LOG_TABLE} FOR INSERT WITH CHECK (true)"
    ))
    .execute(&owner)
    .await?;
    sqlx::query("CREATE OR REPLACE FUNCTION graphql_orm.record_change_journal_entry() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, pg_temp AS $$ BEGIN RETURN NULL; END $$")
        .execute(&owner)
        .await?;
    let journal = journal_errors_on(&owner, CHANGE_LOG_TABLE).await?;
    assert!(
        journal
            .iter()
            .any(|message| message.contains("Unexpected RLS policy")),
        "{journal:?}"
    );
    assert!(
        journal.iter().any(|message| message.contains("differs")),
        "{journal:?}"
    );
    // A modified recorder also fails ORM writes closed.
    let altered = schema
        .execute(
            Request::new(
                "mutation { createRlsCovDoc(input: { tenantId: \"tenant-a\", title: \"Altered\", body: \"x\" }) { success } }",
            )
            .data(tenant_a.clone()),
        )
        .await;
    assert!(!altered.errors.is_empty(), "{:?}", altered.data);

    // Re-applying the schema target repairs every drift, and drops the
    // earlier INSERT policy.
    reapply_target(&owner, "journal").await?;
    for table in [CHANGE_LOG_TABLE, "rls_cov_docs", "rls_cov_plain"] {
        let errors = journal_errors_on(&owner, table).await?;
        assert!(errors.is_empty(), "{table}: {errors:?}");
    }
    let direct = execute_as(&runtime, &direct_insert, &tenant_b).await;
    assert!(direct.is_err(), "{direct:?}");
    let created = schema
        .execute(Request::new(create_plain).data(tenant_a.clone()))
        .await;
    assert!(created.errors.is_empty(), "{:?}", created.errors);
    let plain_id = created.data.into_json()?["createRlsCovPlain"]["rlsCovPlain"]["id"]
        .as_str()
        .expect("plain id")
        .to_string();
    assert_eq!(
        journal_entries(&owner, &plain_id).await?,
        vec![(
            "RlsCovPlain".to_string(),
            "rls_cov_plain".to_string(),
            "create".to_string()
        )]
    );
    Ok(())
}

/// Dependent deletes through the privileged delete pool journal the cascaded
/// child, and the dependent-delete trigger check accepts the recorder.
#[cfg(feature = "change-journal")]
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn dependent_deletes_journal_through_the_recorder() -> Result<(), Box<dyn std::error::Error>>
{
    let Provisioned { owner } = provision().await?;
    let (_, runtime) = role_pool(&owner, "journal_runtime", "NOSUPERUSER NOBYPASSRLS").await?;
    let (_, bypass) = role_pool(&owner, "journal_bypass", "NOSUPERUSER BYPASSRLS").await?;
    let journaled = |pool: sqlx::PgPool| {
        let registry = StaticDependentDeletes::<PostgresBackend>::new(
            RuntimeDependentLimits::new(10, 4).expect("limits"),
        )
        .register::<RlsCovFolder>()
        .expect("register folder")
        .register::<RlsCovFile>()
        .expect("register file");
        let database = graphql_orm::db::Database::builder(pool)
            .change_journal_enabled(true)
            .build();
        let schema: CoverageSchema = schema_builder(database)
            .data("test-user".to_string())
            .data(registry)
            .finish();
        schema
    };
    let schema = journaled(runtime);
    let delete_schema = journaled(bypass);
    let tenant_a = tenant("tenant-a");
    for statement in [
        "mutation { createRlsCovFolder(input: { id: \"jf1\", label: \"Journal folder\" }) { success } }",
        "mutation { createRlsCovFile(input: { id: \"jc1\", tenantId: \"tenant-a\", folderId: \"jf1\", note: \"journal child\" }) { success } }",
    ] {
        let response = schema
            .execute(Request::new(statement).data(tenant_a.clone()))
            .await;
        assert!(
            response.errors.is_empty(),
            "{statement}: {:?}",
            response.errors
        );
    }
    let deleted = delete_schema
        .execute(
            Request::new("mutation { deleteRlsCovFolder(id: \"jf1\") { success } }")
                .data(tenant_a.clone()),
        )
        .await;
    assert!(deleted.errors.is_empty(), "{:?}", deleted.errors);
    assert_eq!(
        journal_entries(&owner, "jf1").await?,
        vec![
            (
                "RlsCovFolder".to_string(),
                "rls_cov_folders".to_string(),
                "create".to_string()
            ),
            (
                "RlsCovFolder".to_string(),
                "rls_cov_folders".to_string(),
                "delete".to_string()
            ),
        ]
    );
    assert_eq!(
        journal_entries(&owner, "jc1").await?,
        vec![
            (
                "RlsCovFile".to_string(),
                "rls_cov_files".to_string(),
                "create".to_string()
            ),
            (
                "RlsCovFile".to_string(),
                "rls_cov_files".to_string(),
                "delete".to_string()
            ),
        ]
    );
    Ok(())
}

/// Run `statements` as `owner` in one transaction with recording on.
#[cfg(feature = "change-journal")]
async fn record_as(pool: &sqlx::PgPool, statements: &[&str]) -> Result<(), sqlx::Error> {
    let mut all = vec!["SET LOCAL graphql_orm.change_journal = 'on'"];
    all.extend_from_slice(statements);
    script_as(pool, &all, None).await
}

/// The recorder check for `RlsCovPlain` in a fresh transaction on `database`,
/// with `setup` statements run first.
#[cfg(feature = "change-journal")]
async fn recorder_check(
    database: &graphql_orm::db::Database<PostgresBackend>,
    setup: &[&str],
) -> Result<Result<(), String>, Box<dyn std::error::Error>> {
    use graphql_orm::graphql::orm::Entity;
    let mut context = graphql_orm::graphql::orm::MutationContext::<PostgresBackend>::begin(
        database,
        graphql_orm::graphql::orm::TransactionMode::Default,
    )
    .await?;
    for statement in setup {
        context.execute(statement, &[]).await?;
    }
    let outcome = context
        .check_change_journal_recorder(<RlsCovPlain as Entity>::metadata())
        .await
        .map_err(|error| error.to_string());
    context.rollback().await?;
    Ok(outcome)
}

/// Grants that would let a role run code as the journal owner are refused:
/// a trigger on the journal would run as its owner whenever the recorder
/// appends an entry.
#[cfg(feature = "change-journal")]
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn journal_privileges_beyond_select_are_refused() -> Result<(), Box<dyn std::error::Error>> {
    let Provisioned { owner } = provision().await?;
    let (role, runtime) = role_pool(&owner, "journal_grants", "NOSUPERUSER NOBYPASSRLS").await?;
    let runtime_database = graphql_orm::db::Database::builder(runtime.clone())
        .change_journal_enabled(true)
        .build();
    let owner_database = graphql_orm::db::Database::builder(owner.clone())
        .change_journal_enabled(true)
        .build();
    let schema: CoverageSchema = schema_builder(runtime_database.clone())
        .data("test-user".to_string())
        .finish();
    let tenant_a = tenant("tenant-a");
    let create_plain = |label: &str| {
        format!("mutation {{ createRlsCovPlain(input: {{ label: \"{label}\" }}) {{ success }} }}")
    };
    assert_eq!(recorder_check(&runtime_database, &[]).await?, Ok(()));

    // INSERT on the journal: RLS still refuses the row, and journaled ORM
    // writes fail closed while the grant exists.
    sqlx::query(&format!("GRANT INSERT ON {CHANGE_LOG_TABLE} TO {role}"))
        .execute(&owner)
        .await?;
    let direct = execute_as(
        &runtime,
        &format!(
            "INSERT INTO {CHANGE_LOG_TABLE} (id, entity_name, table_name, primary_key, action, changed_at) \
             VALUES (gen_random_uuid(), 'RlsCovDoc', 'rls_cov_docs', 'any', 'delete', 1)"
        ),
        &tenant_a,
    )
    .await;
    assert!(direct.is_err(), "{direct:?}");
    let refused = recorder_check(&runtime_database, &[]).await?;
    assert!(
        matches!(&refused, Err(message) if message.contains("privilege other than SELECT")),
        "{refused:?}"
    );
    let write = schema
        .execute(Request::new(create_plain("insert grant")).data(tenant_a.clone()))
        .await;
    assert!(!write.errors.is_empty(), "{:?}", write.data);
    let writers = journal_errors_on(&owner, CHANGE_LOG_TABLE).await?;
    assert!(
        writers
            .iter()
            .any(|message| message.contains(&role) && message.contains("other than SELECT")),
        "{writers:?}"
    );
    sqlx::query(&format!("REVOKE INSERT ON {CHANGE_LOG_TABLE} FROM {role}"))
        .execute(&owner)
        .await?;

    // Column-level grants count too.
    sqlx::query(&format!(
        "GRANT INSERT (id, entity_name, table_name, primary_key, action, changed_at), UPDATE (action) ON {CHANGE_LOG_TABLE} TO {role}"
    ))
    .execute(&owner)
    .await?;
    let column = recorder_check(&runtime_database, &[]).await?;
    assert!(
        matches!(&column, Err(message) if message.contains("privilege other than SELECT")),
        "{column:?}"
    );
    let column_writers = journal_errors_on(&owner, CHANGE_LOG_TABLE).await?;
    assert!(
        column_writers
            .iter()
            .any(|message| message.contains(&role) && message.contains("other than SELECT")),
        "{column_writers:?}"
    );
    reapply_target(&owner, "journal-column-grants").await?;
    let column_held: bool = sqlx::query_scalar(&format!(
        "SELECT has_any_column_privilege('{role}', '{CHANGE_LOG_TABLE}', 'INSERT, UPDATE')"
    ))
    .fetch_one(&owner)
    .await?;
    assert!(!column_held);
    assert_eq!(recorder_check(&runtime_database, &[]).await?, Ok(()));

    // A role acting as the journal owner passes the privilege check, so
    // validation reports it; the runtime role is not reported.
    let as_owner = errors_on(&owner, CHANGE_LOG_TABLE).await?;
    assert!(
        as_owner
            .iter()
            .any(|message| message.contains("acts as the journal owner")),
        "{as_owner:?}"
    );
    let as_runtime = runtime_database
        .schema()
        .validate_target(&graphql_orm_schema_target())
        .await?;
    assert!(
        !as_runtime
            .rls_diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("acts as the journal owner")),
        "{:#?}",
        as_runtime.rls_diagnostics
    );

    // TRIGGER on the journal: a trigger whose function lives in the role's
    // own temporary schema is refused before it can run as the owner.
    sqlx::query(&format!("GRANT TRIGGER ON {CHANGE_LOG_TABLE} TO {role}"))
        .execute(&owner)
        .await?;
    let mut session = runtime.acquire().await?;
    sqlx::raw_sql(&format!(
        "CREATE FUNCTION pg_temp.journal_probe() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$; \
         CREATE TRIGGER journal_probe AFTER INSERT ON {CHANGE_LOG_TABLE} FOR EACH ROW EXECUTE FUNCTION pg_temp.journal_probe()"
    ))
    .execute(&mut *session)
    .await?;
    let raw = record_as(
        &owner,
        &["INSERT INTO rls_cov_plain (id, label) VALUES ('journal-trigger', 'raw')"],
    )
    .await;
    assert!(
        matches!(&raw, Err(error) if error.to_string().contains("carries a trigger or rule")),
        "{raw:?}"
    );
    assert!(journal_entries(&owner, "journal-trigger").await?.is_empty());
    let owner_check = recorder_check(&owner_database, &[]).await?;
    assert!(
        matches!(&owner_check, Err(message) if message.contains("carries a trigger or rule")),
        "{owner_check:?}"
    );
    let write = schema
        .execute(Request::new(create_plain("trigger grant")).data(tenant_a.clone()))
        .await;
    assert!(!write.errors.is_empty(), "{:?}", write.data);
    let detected = journal_errors_on(&owner, CHANGE_LOG_TABLE).await?;
    assert!(
        detected
            .iter()
            .any(|message| message.contains("journal_probe")),
        "{detected:?}"
    );
    assert!(
        detected
            .iter()
            .any(|message| message.contains(&role) && message.contains("other than SELECT")),
        "{detected:?}"
    );
    sqlx::query(&format!("DROP TRIGGER journal_probe ON {CHANGE_LOG_TABLE}"))
        .execute(&owner)
        .await?;
    drop(session);

    // Re-applying the target revokes every journal grant beyond SELECT. The
    // role's SELECT grant survives and is accepted: it cannot write or add
    // triggers, and under RLS with no policies it reads nothing.
    reapply_target(&owner, "journal-grants").await?;
    let privilege = |names: &'static str| {
        format!("SELECT has_table_privilege('{role}', '{CHANGE_LOG_TABLE}', '{names}')")
    };
    let writes: bool = sqlx::query_scalar(&privilege(
        "INSERT, UPDATE, DELETE, TRUNCATE, REFERENCES, TRIGGER",
    ))
    .fetch_one(&owner)
    .await?;
    assert!(!writes);
    let reads: bool = sqlx::query_scalar(&privilege("SELECT"))
        .fetch_one(&owner)
        .await?;
    assert!(reads);
    let journal_count = format!("SELECT COUNT(*)::BIGINT AS count FROM {CHANGE_LOG_TABLE}");
    assert_eq!(
        count_as(&runtime, &journal_count, Some(&tenant_a)).await?,
        0
    );
    let clean = journal_errors_on(&owner, CHANGE_LOG_TABLE).await?;
    assert!(clean.is_empty(), "{clean:?}");
    assert_eq!(recorder_check(&runtime_database, &[]).await?, Ok(()));
    let write = schema
        .execute(Request::new(create_plain("after revoke")).data(tenant_a.clone()))
        .await;
    assert!(write.errors.is_empty(), "{:?}", write.errors);
    Ok(())
}

/// The recorder only records for its own trigger on the table it was
/// installed on, keyed by that table's primary key.
#[cfg(feature = "change-journal")]
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn recorder_refuses_triggers_that_do_not_match_their_table()
-> Result<(), Box<dyn std::error::Error>> {
    let Provisioned { owner } = provision().await?;
    let (role, runtime) = role_pool(&owner, "journal_trigger", "NOSUPERUSER NOBYPASSRLS").await?;
    let runtime_database = graphql_orm::db::Database::builder(runtime.clone())
        .change_journal_enabled(true)
        .build();
    let tenant_a = tenant("tenant-a");
    let recorder = "graphql_orm.record_change_journal_entry";

    // EXECUTE on the recorder is reported and fails journaled writes closed.
    sqlx::query(&format!("GRANT EXECUTE ON FUNCTION {recorder}() TO {role}"))
        .execute(&owner)
        .await?;
    let refused = recorder_check(&runtime_database, &[]).await?;
    assert!(
        matches!(&refused, Err(message) if message.contains("EXECUTE")),
        "{refused:?}"
    );
    let executors = journal_errors_on(&owner, CHANGE_LOG_TABLE).await?;
    assert!(
        executors
            .iter()
            .any(|message| message.contains(&role) && message.contains("EXECUTE")),
        "{executors:?}"
    );

    // With TRIGGER on an owner-owned table, the role can attach the recorder
    // under another name with another entity's arguments. The recorder
    // refuses it, and validation reports it.
    sqlx::query(&format!("GRANT TRIGGER ON rls_cov_plain TO {role}"))
        .execute(&owner)
        .await?;
    script_as(
        &runtime,
        &[&format!(
            "CREATE TRIGGER journal_copy AFTER INSERT OR UPDATE OR DELETE ON rls_cov_plain FOR EACH ROW EXECUTE FUNCTION {recorder}('public', 'public', 'rls_cov_plain', 'RlsCovDoc', 'rls_cov_docs', 'id', 'text')"
        )],
        Some(&tenant_a),
    )
    .await?;
    let copied = script_as(
        &runtime,
        &[
            "SET LOCAL graphql_orm.change_journal = 'on'",
            "INSERT INTO rls_cov_plain (id, label) VALUES ('copied-key', 'x')",
            "DELETE FROM rls_cov_plain WHERE id = 'copied-key'",
        ],
        Some(&tenant_a),
    )
    .await;
    assert!(
        matches!(&copied, Err(error) if error.to_string().contains("misconfigured")),
        "{copied:?}"
    );
    assert!(journal_entries(&owner, "copied-key").await?.is_empty());
    // Triggers found by catalog scan are keyed by their quoted schema and name.
    let other = journal_errors_on(&owner, "\"public\".\"rls_cov_plain\"").await?;
    assert!(
        other.iter().any(|message| message.contains("journal_copy")),
        "{other:?}"
    );
    let shadowed = recorder_check(&runtime_database, &[]).await?;
    assert!(shadowed.is_err(), "{shadowed:?}");

    // A temporary table the role owns is refused even under the ORM's name.
    let temporary = script_as(
        &runtime,
        &[
            "CREATE TEMP TABLE session_rows (id text PRIMARY KEY) ON COMMIT DROP",
            &format!(
                "CREATE TRIGGER graphql_orm_change_journal AFTER INSERT OR UPDATE OR DELETE ON session_rows FOR EACH ROW EXECUTE FUNCTION {recorder}('public', 'pg_temp', 'session_rows', 'RlsCovDoc', 'rls_cov_docs', 'id', 'text')"
            ),
            "SET LOCAL graphql_orm.change_journal = 'on'",
            "INSERT INTO session_rows VALUES ('temporary-key')",
        ],
        Some(&tenant_a),
    )
    .await;
    assert!(temporary.is_err(), "{temporary:?}");
    assert!(journal_entries(&owner, "temporary-key").await?.is_empty());

    // Under the ORM's name, the recorded table and the key column must match
    // the table the trigger fires on.
    for arguments in [
        "'public', 'public', 'rls_cov_docs', 'RlsCovDoc', 'rls_cov_docs', 'id', 'text'",
        "'public', 'public', 'rls_cov_plain', 'RlsCovPlain', 'rls_cov_plain', 'label', 'text'",
    ] {
        sqlx::raw_sql(&format!(
            "DROP TRIGGER IF EXISTS graphql_orm_change_journal ON rls_cov_plain; \
             CREATE TRIGGER graphql_orm_change_journal AFTER INSERT OR UPDATE OR DELETE ON rls_cov_plain FOR EACH ROW EXECUTE FUNCTION {recorder}({arguments})"
        ))
        .execute(&owner)
        .await?;
        let mismatched = record_as(
            &owner,
            &["INSERT INTO rls_cov_plain (id, label) VALUES ('mismatched-key', 'x')"],
        )
        .await;
        assert!(
            matches!(&mismatched, Err(error) if error.to_string().contains("does not match the table")),
            "{arguments}: {mismatched:?}"
        );
        assert!(journal_entries(&owner, "mismatched-key").await?.is_empty());
        let drift = journal_errors_on(&owner, "rls_cov_plain").await?;
        assert!(!drift.is_empty(), "{arguments}");
    }

    // Under the ORM's name on an owner-owned table outside the target, the
    // declared table must still be the table the trigger fires on.
    sqlx::raw_sql(&format!(
        "CREATE SCHEMA journal_aux; CREATE TABLE journal_aux.side (id text PRIMARY KEY, note text); \
         GRANT USAGE ON SCHEMA journal_aux TO {role}; \
         GRANT INSERT, DELETE, TRIGGER ON journal_aux.side TO {role}"
    ))
    .execute(&owner)
    .await?;
    script_as(
        &runtime,
        &[&format!(
            "CREATE TRIGGER graphql_orm_change_journal AFTER INSERT OR UPDATE OR DELETE ON journal_aux.side FOR EACH ROW EXECUTE FUNCTION {recorder}('public', 'journal_aux', 'side', 'RlsCovDoc', 'rls_cov_docs', 'id', 'text')"
        )],
        Some(&tenant_a),
    )
    .await?;
    let side = script_as(
        &runtime,
        &[
            "SET LOCAL graphql_orm.change_journal = 'on'",
            "INSERT INTO journal_aux.side (id) VALUES ('side-key')",
            "DELETE FROM journal_aux.side WHERE id = 'side-key'",
        ],
        Some(&tenant_a),
    )
    .await;
    assert!(
        matches!(&side, Err(error) if error.to_string().contains("does not match the table")),
        "{side:?}"
    );
    assert!(journal_entries(&owner, "side-key").await?.is_empty());

    // Re-applying the target restores the trigger and revokes EXECUTE. The
    // trigger it does not manage stays, and validation keeps reporting it.
    reapply_target(&owner, "journal-trigger").await?;
    let copies: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pg_trigger WHERE tgrelid = 'rls_cov_plain'::regclass AND tgname = 'journal_copy'",
    )
    .fetch_one(&owner)
    .await?;
    assert_eq!(
        copies, 0,
        "the plan drops recorder triggers under other names"
    );
    // It can only record its own table's changes, so another schema target
    // may own it: a warning, not an error.
    let report = graphql_orm::db::Database::new(owner.clone())
        .schema()
        .validate_target(&graphql_orm_schema_target())
        .await?;
    let stray = report
        .rls_diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.table.as_deref() == Some("\"journal_aux\".\"side\""))
        .collect::<Vec<_>>();
    assert!(
        stray.iter().any(|diagnostic| {
            diagnostic.severity == SchemaDiagnosticSeverity::Warning
                && diagnostic
                    .message
                    .contains("not on a single-key table of this target")
        }),
        "{stray:#?}"
    );
    sqlx::query("DROP SCHEMA journal_aux CASCADE")
        .execute(&owner)
        .await?;
    for table in [CHANGE_LOG_TABLE, "rls_cov_plain"] {
        let errors = journal_errors_on(&owner, table).await?;
        assert!(errors.is_empty(), "{table}: {errors:?}");
    }
    sqlx::query(&format!("REVOKE TRIGGER ON rls_cov_plain FROM {role}"))
        .execute(&owner)
        .await?;
    assert_eq!(recorder_check(&runtime_database, &[]).await?, Ok(()));
    Ok(())
}

/// A key change journals both keys; the check names a switched-off journal
/// and a replica session instead of reporting a missing recorder.
#[cfg(feature = "change-journal")]
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn key_changes_and_inactive_recording_are_reported() -> Result<(), Box<dyn std::error::Error>>
{
    let Provisioned { owner } = provision().await?;
    let (_, runtime) = role_pool(&owner, "journal_keys", "NOSUPERUSER NOBYPASSRLS").await?;
    let runtime_database = graphql_orm::db::Database::builder(runtime.clone())
        .change_journal_enabled(true)
        .build();
    let mut context = graphql_orm::graphql::orm::MutationContext::<PostgresBackend>::begin(
        &runtime_database,
        graphql_orm::graphql::orm::TransactionMode::Default,
    )
    .await?;
    context
        .execute(
            "INSERT INTO rls_cov_plain (id, label) VALUES ('key-old', 'moving')",
            &[],
        )
        .await?;
    context
        .execute(
            "UPDATE rls_cov_plain SET id = 'key-new' WHERE id = 'key-old'",
            &[],
        )
        .await?;
    context
        .execute(
            "UPDATE rls_cov_plain SET label = 'moved' WHERE id = 'key-new'",
            &[],
        )
        .await?;
    context.commit_and_emit().await?;
    let plain = |action: &str| {
        (
            "RlsCovPlain".to_string(),
            "rls_cov_plain".to_string(),
            action.to_string(),
        )
    };
    assert_eq!(
        journal_entries(&owner, "key-old").await?,
        vec![plain("create"), plain("delete")]
    );
    assert_eq!(
        journal_entries(&owner, "key-new").await?,
        vec![plain("create"), plain("update")]
    );

    // Replica sessions skip ordinary triggers, so the check refuses them.
    let owner_database = graphql_orm::db::Database::builder(owner.clone())
        .change_journal_enabled(true)
        .build();
    let replica = recorder_check(
        &owner_database,
        &["SET LOCAL session_replication_role = replica"],
    )
    .await?;
    assert!(
        matches!(&replica, Err(message) if message.contains("session_replication_role is replica")),
        "{replica:?}"
    );
    let switched_off = recorder_check(
        &owner_database,
        &["SET LOCAL graphql_orm.change_journal = 'off'"],
    )
    .await?;
    assert!(
        matches!(&switched_off, Err(message) if message.contains("recording is not enabled")),
        "{switched_off:?}"
    );
    let unjournaled = recorder_check(&graphql_orm::db::Database::new(owner.clone()), &[]).await?;
    assert!(
        matches!(&unjournaled, Err(message) if message.contains("recording is not enabled")),
        "{unjournaled:?}"
    );
    assert_eq!(recorder_check(&owner_database, &[]).await?, Ok(()));
    Ok(())
}

/// The registry that binds declared tables to the recorder can only be
/// changed by its owner. A grant, a foreign owner, a changed binding or a
/// missing registry each fail journaled writes closed, make the recorder
/// raise, and are reported; re-applying the target repairs them.
#[cfg(feature = "change-journal")]
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn change_journal_registry_protections_are_enforced() -> Result<(), Box<dyn std::error::Error>>
{
    const REGISTRY: &str = "graphql_orm.change_journal_tables";
    let Provisioned { owner } = provision().await?;
    let (role, runtime) = role_pool(&owner, "journal_registry", "NOSUPERUSER NOBYPASSRLS").await?;
    let runtime_database = graphql_orm::db::Database::builder(runtime.clone())
        .change_journal_enabled(true)
        .build();
    let raw_insert =
        |key: &str| format!("INSERT INTO rls_cov_plain (id, label) VALUES ('{key}', 'x')");
    let all_messages = |report: &graphql_orm::graphql::orm::SchemaTargetValidationReport| {
        report
            .rls_diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == SchemaDiagnosticSeverity::Error)
            .map(|diagnostic| diagnostic.message.clone())
            .collect::<Vec<_>>()
    };
    let validate = || async {
        graphql_orm::db::Database::new(owner.clone())
            .schema()
            .validate_target(&graphql_orm_schema_target())
            .await
    };
    assert_eq!(recorder_check(&runtime_database, &[]).await?, Ok(()));

    // Writes to the registry by another role, at table or column level.
    for (step, grant, held) in [
        (
            "registry-insert",
            format!("GRANT INSERT ON {REGISTRY} TO {role}"),
            format!("has_table_privilege('{role}', '{REGISTRY}', 'INSERT')"),
        ),
        (
            "registry-update-column",
            format!("GRANT UPDATE (relid) ON {REGISTRY} TO {role}"),
            format!("has_column_privilege('{role}', '{REGISTRY}', 'relid', 'UPDATE')"),
        ),
        (
            "registry-trigger",
            format!("GRANT TRIGGER ON {REGISTRY} TO {role}"),
            format!("has_table_privilege('{role}', '{REGISTRY}', 'TRIGGER')"),
        ),
    ] {
        sqlx::query(&grant).execute(&owner).await?;
        let refused = recorder_check(&runtime_database, &[]).await?;
        assert!(
            matches!(&refused, Err(message) if message.contains("registry")),
            "{step}: {refused:?}"
        );
        let raised = record_as(&owner, &[&raw_insert(step)]).await;
        assert!(
            matches!(&raised, Err(error) if error.to_string().contains("registry is missing, not owned")),
            "{step}: {raised:?}"
        );
        assert!(journal_entries(&owner, step).await?.is_empty());
        let reported = all_messages(&validate().await?);
        assert!(
            reported.iter().any(|message| message.contains(&role)
                && message.contains("other than SELECT on graphql_orm.change_journal_tables")),
            "{step}: {reported:?}"
        );
        reapply_target(&owner, step).await?;
        let still: bool = sqlx::query_scalar(&format!("SELECT {held}"))
            .fetch_one(&owner)
            .await?;
        assert!(!still, "{step}: the plan revokes the grant");
        assert_eq!(
            recorder_check(&runtime_database, &[]).await?,
            Ok(()),
            "{step}"
        );
    }

    // A registry owned by another role than the recorder's owner.
    sqlx::raw_sql(&format!(
        "CREATE ROLE journal_registry_owner_{role}; ALTER TABLE {REGISTRY} OWNER TO journal_registry_owner_{role}"
    ))
    .execute(&owner)
    .await?;
    let foreign = recorder_check(&runtime_database, &[]).await?;
    assert!(
        matches!(&foreign, Err(message) if message.contains("not owned by the recorder's owner")),
        "{foreign:?}"
    );
    let raised = record_as(&owner, &[&raw_insert("registry-owner")]).await;
    assert!(
        matches!(&raised, Err(error) if error.to_string().contains("registry is missing, not owned")),
        "{raised:?}"
    );
    let reported = all_messages(&validate().await?);
    assert!(
        reported
            .iter()
            .any(|message| message.contains("is not owned by the owner of graphql_orm.")),
        "{reported:?}"
    );
    sqlx::query(&format!("ALTER TABLE {REGISTRY} OWNER TO CURRENT_USER"))
        .execute(&owner)
        .await?;
    assert_eq!(recorder_check(&runtime_database, &[]).await?, Ok(()));

    // A binding with another entity or key column.
    for (column, value, message) in [
        ("entity_name", "RlsCovDoc", "to entity RlsCovDoc by key id"),
        ("key_column", "label", "to entity RlsCovPlain by key label"),
    ] {
        sqlx::query(&format!(
            "UPDATE {REGISTRY} SET {column} = '{value}' WHERE declared_table = 'rls_cov_plain'"
        ))
        .execute(&owner)
        .await?;
        let refused = recorder_check(&runtime_database, &[]).await?;
        assert!(
            matches!(&refused, Err(text) if text.contains("registry binding")),
            "{column}: {refused:?}"
        );
        let key = format!("binding-{column}");
        let raised = record_as(&owner, &[&raw_insert(&key)]).await;
        assert!(
            matches!(&raised, Err(error) if error.to_string().contains("does not match the table")),
            "{column}: {raised:?}"
        );
        assert!(journal_entries(&owner, &key).await?.is_empty());
        let reported = all_messages(&validate().await?);
        assert!(
            reported.iter().any(|text| text.contains(message)),
            "{column}: {reported:?}"
        );
        reapply_target(&owner, &format!("registry-{column}")).await?;
        assert_eq!(
            recorder_check(&runtime_database, &[]).await?,
            Ok(()),
            "{column}"
        );
    }

    // Without USAGE on the schema the check says so, instead of failing on a
    // raw lookup.
    sqlx::raw_sql(&format!(
        "REVOKE USAGE ON SCHEMA graphql_orm FROM PUBLIC; REVOKE USAGE ON SCHEMA graphql_orm FROM {role}"
    ))
    .execute(&owner)
    .await?;
    let no_usage = recorder_check(&runtime_database, &[]).await?;
    assert!(
        matches!(&no_usage, Err(message) if message.contains("no USAGE on schema graphql_orm")),
        "{no_usage:?}"
    );
    // Re-applying grants USAGE to PUBLIC, which is all the runtime role needs.
    reapply_target(&owner, "registry-usage").await?;
    assert_eq!(recorder_check(&runtime_database, &[]).await?, Ok(()));

    // A missing registry, as before the plan is applied.
    sqlx::query(&format!("DROP TABLE {REGISTRY}"))
        .execute(&owner)
        .await?;
    let missing = recorder_check(&runtime_database, &[]).await?;
    assert!(
        matches!(&missing, Err(message) if message.contains("registry graphql_orm.change_journal_tables does not exist")),
        "{missing:?}"
    );
    let reported = all_messages(&validate().await?);
    assert!(
        reported
            .iter()
            .any(|message| message.contains("Missing change journal registry")),
        "{reported:?}"
    );
    reapply_target(&owner, "registry-missing").await?;
    assert_eq!(recorder_check(&runtime_database, &[]).await?, Ok(()));
    Ok(())
}
