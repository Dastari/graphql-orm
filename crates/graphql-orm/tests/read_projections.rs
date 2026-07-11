#![cfg(any(feature = "sqlite", feature = "postgres"))]

use graphql_orm::prelude::*;

#[derive(
    GraphQLEntity, GraphQLOperations, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize,
)]
#[graphql_entity(
    table = "projection_certificates",
    plural = "ProjectionCertificates",
    default_sort = "serial ASC",
    read_policy = "certificates.read"
)]
#[graphql_orm(projection(
    name = "PublicCertificateProjection",
    fields = [
        id,
        role,
        serial,
        spki_digest,
        pem,
        parent_id,
        issued_at
    ],
    private = true
))]
#[graphql_orm(projection(
    name = "SensitiveCertificateProjection",
    fields = [id, private_key_enc],
    private = true
))]
struct ProjectionCertificate {
    #[primary_key]
    #[filterable(type = "uuid")]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    #[sortable]
    role: String,
    #[unique]
    #[filterable(type = "string")]
    #[sortable]
    serial: String,
    spki_digest: Vec<u8>,
    pem: String,
    parent_id: Option<graphql_orm::uuid::Uuid>,
    #[filterable(type = "number")]
    #[sortable]
    issued_at: i64,
    #[graphql_orm(private, sensitive)]
    #[backup(redact)]
    private_key_enc: String,
}

schema_roots! {
    query_custom_ops: [],
    entities: [ProjectionCertificate],
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, PartialEq)]
#[graphql_entity(
    table = "projection_probe",
    plural = "ProjectionProbes",
    schema_policy = "external_read_only"
)]
#[graphql_orm(projection(
    name = "SafeProbeProjection",
    fields = [id, safe_value],
    private = true
))]
struct ProjectionProbe {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    safe_value: String,
    secret_value: String,
}

#[cfg(feature = "postgres")]
#[derive(GraphQLEntity, Clone, Debug, PartialEq)]
#[graphql_entity(
    backend = "postgres",
    table = "projection_tenant_secrets",
    plural = "ProjectionTenantSecrets",
    schema_policy = "external_read_only"
)]
#[graphql_orm(projection(
    name = "TenantPublicProjection",
    fields = [id, tenant_id, label],
    private = true
))]
struct ProjectionTenantSecret {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    tenant_id: String,
    label: String,
    #[graphql_orm(private, sensitive)]
    secret: String,
}

#[derive(Clone, Default)]
struct AllowEntityReads;

impl EntityPolicy for AllowEntityReads {
    fn can_access_entity<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _kind: EntityAccessKind,
        _surface: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(true) })
    }
}

#[derive(Clone, Default)]
struct DenyRows;

impl RowPolicy for DenyRows {
    fn can_read_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _surface: EntityAccessSurface,
        _row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }

    fn can_write_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _surface: EntityAccessSurface,
        _row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
}

#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;

async fn database() -> Result<Database<Backend>, Box<dyn std::error::Error>> {
    #[cfg(feature = "sqlite")]
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    #[cfg(feature = "postgres")]
    let database = {
        let url = std::env::var("TEST_DATABASE_URL")?;
        let database = Database::<PostgresBackend>::connect_postgres(url).await?;
        graphql_orm::sqlx::query("DROP VIEW IF EXISTS projection_probe CASCADE")
            .execute(database.pool())
            .await?;
        graphql_orm::sqlx::query("DROP TABLE IF EXISTS projection_probe_base CASCADE")
            .execute(database.pool())
            .await?;
        graphql_orm::sqlx::query("DROP TABLE IF EXISTS projection_certificates CASCADE")
            .execute(database.pool())
            .await?;
        graphql_orm::sqlx::query("DROP FUNCTION IF EXISTS projection_forbidden_secret()")
            .execute(database.pool())
            .await?;
        database
    };
    Ok(database)
}

fn create_input(serial: &str, issued_at: i64) -> CreateProjectionCertificateInput {
    CreateProjectionCertificateInput {
        role: "intermediate".to_string(),
        serial: serial.to_string(),
        spki_digest: vec![0xAA, 0xBB, 0xCC],
        pem: format!("-----BEGIN CERTIFICATE-----{serial}"),
        parent_id: None,
        issued_at,
        private_key_enc: format!("encrypted:{serial}"),
    }
}

#[tokio::test]
async fn projections_are_typed_bounded_transaction_visible_and_authorized()
-> Result<(), Box<dyn std::error::Error>> {
    if cfg!(feature = "postgres") && std::env::var("TEST_DATABASE_URL").is_err() {
        return Ok(());
    }
    let mut database = database().await?;
    database.set_entity_policy(AllowEntityReads);
    database.set_authorization_mode(AuthorizationMode::DeclaredPoliciesRequired);
    database.set_pagination_config(
        PaginationConfig::secure()
            .with_default_limit(Some(2))
            .with_max_limit(Some(2)),
    );
    let sdl = schema_builder(database.clone()).finish().sdl();
    assert!(!sdl.contains("PublicCertificateProjection"));
    assert!(!sdl.contains("SensitiveCertificateProjection"));
    assert!(!sdl.contains("privateKeyEnc"));
    let entities = [ProjectionCertificate::metadata()];
    let version = format!("projection-init-{}", graphql_orm::uuid::Uuid::new_v4());
    let plan = database
        .schema()
        .plan_migration_to_entities_with_options(
            version,
            "typed projections",
            &entities,
            PlanOptions::managed_tables_only(),
        )
        .await?;
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;

    let first = ProjectionCertificate::insert(&database, create_input("serial-1", 10)).await?;
    ProjectionCertificate::insert(&database, create_input("serial-2", 20)).await?;
    ProjectionCertificate::insert(&database, create_input("serial-3", 30)).await?;

    let strict_without_provider = Database::<Backend>::new(database.pool().clone())
        .with_authorization_mode(AuthorizationMode::DeclaredPoliciesRequired);
    let misconfigured = PublicCertificateProjection::query(&strict_without_provider)
        .fetch_first()
        .await
        .expect_err("declared-policy-required projection must require its provider");
    assert!(
        misconfigured
            .to_string()
            .contains("authorization is misconfigured")
    );

    let projected = PublicCertificateProjection::find_by_id(&database, &first.id)
        .await?
        .expect("public certificate projection");
    assert_eq!(projected.serial, "serial-1");
    assert_eq!(projected.spki_digest, vec![0xAA, 0xBB, 0xCC]);
    assert_eq!(projected.parent_id, None);
    assert_eq!(projected.issued_at, 10);
    assert_eq!(
        PublicCertificateProjection::find_by_serial(&database, &"serial-1".to_string())
            .await?
            .expect("unique lookup"),
        projected
    );

    let bounded = PublicCertificateProjection::query(&database)
        .filter(ProjectionCertificateWhereInput {
            role: Some(StringFilter {
                eq: Some("intermediate".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        })
        .order_by(ProjectionCertificateOrderByInput {
            issued_at: Some(OrderDirection::Desc),
            ..Default::default()
        })
        .limit(999)
        .fetch_all()
        .await?;
    assert_eq!(bounded.len(), 2, "projection lists use configured bounds");
    assert_eq!(bounded[0].serial, "serial-3");

    let repository_value = projected.clone();
    let transaction_value = database
        .transaction(TransactionMode::Default, |transaction| {
            let id = first.id;
            Box::pin(async move {
                let projected = PublicCertificateProjection::find_by_id_in(transaction, &id)
                    .await
                    .map_err(OrmPublicError::from)?
                    .expect("transaction projection");
                Ok(projected)
            })
        })
        .await?;
    assert_eq!(transaction_value, repository_value);

    let own_write = database
        .transaction(TransactionMode::StateMachine, |transaction| {
            Box::pin(async move {
                let inserted = transaction
                    .insert::<ProjectionCertificate>(create_input("serial-own-write", 40))
                    .await
                    .map_err(OrmPublicError::from)?;
                transaction
                    .project::<PublicCertificateProjection>()
                    .filter(ProjectionCertificateWhereInput {
                        id: Some(UuidFilter {
                            eq: Some(inserted.id),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })
                    .fetch_optional_one()
                    .await
                    .map_err(OrmPublicError::from)
            })
        })
        .await?
        .expect("own write visible to transaction projection");
    assert_eq!(own_write.serial, "serial-own-write");

    let debug = format!(
        "{:?}",
        SensitiveCertificateProjection {
            id: first.id,
            private_key_enc: "must-not-appear".to_string(),
        }
    );
    assert!(debug.contains("[redacted]"));
    assert!(!debug.contains("must-not-appear"));

    database.set_row_policy(DenyRows);
    let denied = PublicCertificateProjection::query(&database)
        .fetch_first()
        .await
        .expect_err("application row policy cannot be bypassed by a projection");
    assert!(denied.to_string().contains("projection reads are denied"));
    Ok(())
}

#[tokio::test]
async fn backend_observable_probe_proves_excluded_column_is_not_selected()
-> Result<(), Box<dyn std::error::Error>> {
    if cfg!(feature = "postgres") && std::env::var("TEST_DATABASE_URL").is_err() {
        return Ok(());
    }
    let database = database().await?;
    #[cfg(feature = "sqlite")]
    {
        graphql_orm::sqlx::query(
            "CREATE TABLE projection_probe_base (id TEXT PRIMARY KEY, safe_value TEXT NOT NULL)",
        )
        .execute(database.pool())
        .await?;
        graphql_orm::sqlx::query("INSERT INTO projection_probe_base VALUES ('one', 'safe')")
            .execute(database.pool())
            .await?;
        graphql_orm::sqlx::query(
            "CREATE VIEW projection_probe AS
             SELECT id, safe_value, json_extract('not-json', '$.secret') AS secret_value
             FROM projection_probe_base",
        )
        .execute(database.pool())
        .await?;
    }
    #[cfg(feature = "postgres")]
    {
        graphql_orm::sqlx::query(
            "CREATE FUNCTION projection_forbidden_secret() RETURNS TEXT LANGUAGE plpgsql STABLE
             AS $$ BEGIN RAISE EXCEPTION 'secret column was evaluated'; END $$",
        )
        .execute(database.pool())
        .await?;
        graphql_orm::sqlx::query(
            "CREATE TABLE projection_probe_base (id TEXT PRIMARY KEY, safe_value TEXT NOT NULL)",
        )
        .execute(database.pool())
        .await?;
        graphql_orm::sqlx::query("INSERT INTO projection_probe_base VALUES ('one', 'safe')")
            .execute(database.pool())
            .await?;
        graphql_orm::sqlx::query(
            "CREATE VIEW projection_probe AS
             SELECT id, safe_value, projection_forbidden_secret() AS secret_value
             FROM projection_probe_base",
        )
        .execute(database.pool())
        .await?;
    }

    let safe = SafeProbeProjection::find_by_id(&database, &"one".to_string())
        .await?
        .expect("safe projection does not evaluate excluded view expression");
    assert_eq!(safe.safe_value, "safe");
    ProjectionProbe::find_by_id(&database, &"one".to_string())
        .await
        .expect_err("full-entity SELECT evaluates the forbidden secret column");
    Ok(())
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_undecodable_excluded_value_is_never_decoded()
-> Result<(), Box<dyn std::error::Error>> {
    let mut database = database().await?;
    database.set_entity_policy(AllowEntityReads);
    let entities = [ProjectionCertificate::metadata()];
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "projection-undecodable",
            "projection decode boundary",
            &entities,
        )
        .await?;
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    let entity = ProjectionCertificate::insert(&database, create_input("tampered", 99)).await?;
    graphql_orm::sqlx::query(
        "UPDATE projection_certificates SET private_key_enc = X'80' WHERE id = ?",
    )
    .bind(entity.id.to_string())
    .execute(database.pool())
    .await?;
    assert!(
        PublicCertificateProjection::find_by_id(&database, &entity.id)
            .await?
            .is_some()
    );
    ProjectionCertificate::find_by_id(&database, &entity.id)
        .await
        .expect_err("full entity cannot decode tampered sensitive text");
    Ok(())
}

#[cfg(feature = "postgres")]
#[tokio::test]
async fn postgres_transaction_auth_and_rls_filter_projection_rows()
-> Result<(), Box<dyn std::error::Error>> {
    use std::str::FromStr;

    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        return Ok(());
    };
    let owner = graphql_orm::sqlx::PgPool::connect(&url).await?;
    graphql_orm::sqlx::query("DROP TABLE IF EXISTS projection_tenant_secrets CASCADE")
        .execute(&owner)
        .await?;
    graphql_orm::sqlx::query(
        "CREATE TABLE projection_tenant_secrets (
            id TEXT PRIMARY KEY,
            tenant_id TEXT NOT NULL,
            label TEXT NOT NULL,
            secret TEXT NOT NULL
        )",
    )
    .execute(&owner)
    .await?;
    graphql_orm::sqlx::query(
        "INSERT INTO projection_tenant_secrets VALUES
         ('a', 'tenant-a', 'visible', 'secret-a'),
         ('b', 'tenant-b', 'hidden', 'secret-b')",
    )
    .execute(&owner)
    .await?;
    graphql_orm::sqlx::query("ALTER TABLE projection_tenant_secrets ENABLE ROW LEVEL SECURITY")
        .execute(&owner)
        .await?;
    graphql_orm::sqlx::query("ALTER TABLE projection_tenant_secrets FORCE ROW LEVEL SECURITY")
        .execute(&owner)
        .await?;
    graphql_orm::sqlx::query(
        "CREATE POLICY projection_tenant_read ON projection_tenant_secrets
         FOR SELECT USING (tenant_id = current_setting('app.tenant_id', true))",
    )
    .execute(&owner)
    .await?;

    let role = format!(
        "projection_reader_{}",
        graphql_orm::uuid::Uuid::new_v4().simple()
    );
    let password = "projection-test-password";
    graphql_orm::sqlx::query(&format!("CREATE ROLE {role} LOGIN PASSWORD '{password}'"))
        .execute(&owner)
        .await?;
    graphql_orm::sqlx::query(&format!("GRANT USAGE ON SCHEMA public TO {role}"))
        .execute(&owner)
        .await?;
    graphql_orm::sqlx::query(&format!(
        "GRANT SELECT ON projection_tenant_secrets TO {role}"
    ))
    .execute(&owner)
    .await?;

    let options = graphql_orm::sqlx::postgres::PgConnectOptions::from_str(&url)?
        .username(&role)
        .password(password);
    let reader_pool = graphql_orm::sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await?;
    let reader = Database::<PostgresBackend>::new(reader_pool.clone());
    let auth = DbAuthContext {
        user_id: Some("reader".to_string()),
        subject: Some("reader".to_string()),
        tenant_id: Some("tenant-a".to_string()),
        roles: Vec::new(),
        scopes: Vec::new(),
        claims_json: None,
        ..Default::default()
    };
    let visible = reader
        .transaction_with_auth(TransactionMode::Default, Some(&auth), |transaction| {
            Box::pin(async move {
                transaction
                    .project::<TenantPublicProjection>()
                    .fetch_all()
                    .await
                    .map_err(OrmPublicError::from)
            })
        })
        .await?;
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].id, "a");
    assert_eq!(visible[0].label, "visible");

    let without_auth = reader
        .transaction(TransactionMode::Default, |transaction| {
            Box::pin(async move {
                transaction
                    .project::<TenantPublicProjection>()
                    .fetch_all()
                    .await
                    .map_err(OrmPublicError::from)
            })
        })
        .await?;
    assert!(without_auth.is_empty());

    drop(reader);
    reader_pool.close().await;
    graphql_orm::sqlx::query("DROP TABLE projection_tenant_secrets CASCADE")
        .execute(&owner)
        .await?;
    graphql_orm::sqlx::query(&format!("DROP OWNED BY {role}"))
        .execute(&owner)
        .await?;
    graphql_orm::sqlx::query(&format!("DROP ROLE {role}"))
        .execute(&owner)
        .await?;
    Ok(())
}
