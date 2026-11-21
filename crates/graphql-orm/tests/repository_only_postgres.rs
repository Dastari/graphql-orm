#![cfg(feature = "postgres")]
//! Owned disposable PostgreSQL parity for repository-only entities.

use graphql_orm::prelude::*;

#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use owned_postgres::OwnedPostgres;

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

#[tokio::test]
#[ignore = "creates and owns a disposable Docker PostgreSQL container"]
async fn repository_only_postgres_crud_projection_and_serializable_transaction()
-> Result<(), Box<dyn std::error::Error>> {
    let owned = OwnedPostgres::start_postgres_16("repository-only")?;
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
