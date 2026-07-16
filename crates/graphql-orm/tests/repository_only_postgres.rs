#![cfg(feature = "postgres")]
//! Owned disposable PostgreSQL parity for repository-only entities.

use std::process::Command;

use graphql_orm::prelude::*;

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[repository_entity(
    backend = "postgres",
    table = "repository_pg_credentials",
    plural = "RepositoryPgCredentials",
    default_sort = "username ASC"
)]
#[graphql_orm(projection(
    name = "PgCredentialPublicProjection",
    fields = [id, username, status],
    private = true
))]
struct RepositoryPgCredential {
    #[primary_key]
    id: String,
    #[unique]
    #[filterable(type = "string")]
    #[sortable]
    username: String,
    #[filterable(type = "string")]
    status: String,
    #[graphql_orm(private, sensitive)]
    secret_hash: Vec<u8>,
    #[graphql_orm(version, default = "0")]
    version: i64,
}

#[derive(Clone)]
struct AllowEntities;

impl EntityPolicy<PostgresBackend> for AllowEntities {
    fn can_access_entity<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<PostgresBackend>,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _kind: EntityAccessKind,
        _surface: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

struct OwnedPostgres {
    name: String,
    url: String,
}

impl Drop for OwnedPostgres {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "--force", &self.name])
            .output();
    }
}

impl OwnedPostgres {
    fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let name = format!("graphql-orm-repository-only-{suffix}");
        let password = format!("repository_{suffix}");
        let database = format!("repository_{suffix}");
        let output = Command::new("docker")
            .args([
                "run",
                "--detach",
                "--rm",
                "--name",
                &name,
                "--publish",
                "127.0.0.1::5432",
                "--env",
                "POSTGRES_USER=repository_owner",
                "--env",
                &format!("POSTGRES_PASSWORD={password}"),
                "--env",
                &format!("POSTGRES_DB={database}"),
                "postgres:16-alpine",
            ])
            .output()?;
        if !output.status.success() {
            return Err(format!(
                "failed to start disposable PostgreSQL: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        let mut owned = Self {
            name,
            url: String::new(),
        };
        for _ in 0..120 {
            let ready = Command::new("docker")
                .args(["exec", &owned.name, "pg_isready", "-U", "repository_owner"])
                .output()?;
            if ready.status.success() {
                let output = Command::new("docker")
                    .args(["port", &owned.name, "5432/tcp"])
                    .output()?;
                let published = String::from_utf8(output.stdout)?;
                let port = published
                    .lines()
                    .find_map(|line| line.strip_prefix("127.0.0.1:"))
                    .ok_or("Docker did not publish PostgreSQL on loopback")?;
                owned.url =
                    format!("postgres://repository_owner:{password}@127.0.0.1:{port}/{database}");
                // `pg_isready` runs inside the container. Give Docker's
                // loopback-published proxy a short settling window as well,
                // especially while the full PostgreSQL matrix is under load.
                std::thread::sleep(std::time::Duration::from_millis(500));
                return Ok(owned);
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        Err("disposable PostgreSQL did not become ready".into())
    }
}

#[tokio::test]
#[ignore = "creates and owns a disposable Docker PostgreSQL container"]
async fn repository_only_postgres_crud_projection_and_serializable_transaction()
-> Result<(), Box<dyn std::error::Error>> {
    let owned = OwnedPostgres::start()?;
    let mut database = Database::<PostgresBackend>::connect_postgres(&owned.url).await?;
    database.set_authorization_mode(AuthorizationMode::ExplicitPolicyForAllExposedOperations);
    database.set_entity_policy(AllowEntities);
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "repository-only-postgres",
            "repository-only postgres parity",
            &[RepositoryPgCredential::metadata()],
        )
        .await?;
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;

    let created = RepositoryPgCredential::insert(
        &database,
        CreateRepositoryPgCredentialInput {
            username: "alice".to_string(),
            status: "pending".to_string(),
            secret_hash: vec![0, 1, 2, 255],
        },
    )
    .await?;
    let projected = PgCredentialPublicProjection::find_by_id(&database, &created.id)
        .await?
        .ok_or("missing projection")?;
    assert_eq!(projected.username, "alice");
    let filtered = RepositoryPgCredential::query(&database)
        .filter(RepositoryPgCredentialWhereInput {
            username: Some(StringFilter {
                eq: Some("alice".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        })
        .fetch_optional_one()
        .await?
        .ok_or("missing filtered credential")?;
    assert_eq!(filtered.id, created.id);

    let id = created.id.clone();
    let result = database
        .transaction(TransactionMode::StateMachine, |transaction| {
            Box::pin(async move {
                transaction
                    .compare_and_swap::<RepositoryPgCredential>(
                        &id,
                        0,
                        RepositoryPgCredentialWhereInput {
                            status: Some(StringFilter {
                                eq: Some("pending".to_string()),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                        UpdateRepositoryPgCredentialInput {
                            username: None,
                            status: Some("active".to_string()),
                            secret_hash: None,
                        },
                    )
                    .await
                    .map_err(OrmPublicError::from)
            })
        })
        .await?;
    assert!(matches!(result, ConditionalUpdateOutcome::Updated(_)));
    let loaded = RepositoryPgCredential::find_by_id(&database, &created.id)
        .await?
        .ok_or("missing credential")?;
    assert_eq!(loaded.secret_hash, vec![0, 1, 2, 255]);
    assert_eq!(loaded.status, "active");

    let rollback = database
        .transaction(TransactionMode::StateMachine, |transaction| {
            Box::pin(async move {
                transaction
                    .insert::<RepositoryPgCredential>(CreateRepositoryPgCredentialInput {
                        username: "rolled-back".to_string(),
                        status: "pending".to_string(),
                        secret_hash: vec![9],
                    })
                    .await
                    .map_err(OrmPublicError::from)?;
                Err::<(), _>(OrmPublicError::new(OrmErrorCode::Conflict))
            })
        })
        .await;
    assert!(rollback.is_err());
    assert!(
        RepositoryPgCredential::query(&database)
            .filter(RepositoryPgCredentialWhereInput {
                username: Some(StringFilter {
                    eq: Some("rolled-back".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .fetch_first()
            .await?
            .is_none()
    );

    let duplicate = RepositoryPgCredential::insert(
        &database,
        CreateRepositoryPgCredentialInput {
            username: "alice".to_string(),
            status: "duplicate".to_string(),
            secret_hash: vec![3],
        },
    )
    .await;
    assert!(duplicate.is_err(), "unique constraint must fail closed");

    RepositoryPgCredential::delete_by_id(&database, &created.id).await?;
    assert!(
        RepositoryPgCredential::find_by_id(&database, &created.id)
            .await?
            .is_none()
    );
    drop(database);
    drop(owned);
    Ok(())
}
