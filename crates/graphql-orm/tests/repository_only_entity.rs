#![cfg(feature = "sqlite")]

use std::sync::{Arc, Mutex};

use graphql_orm::prelude::*;

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
#[repository_entity(
    backend = "sqlite",
    table = "repository_credentials",
    plural = "RepositoryCredentials",
    default_sort = "username ASC",
    keyset = "username asc, id asc"
)]
#[graphql_orm(search(index = true, tokenizer = "unicode61"))]
#[graphql_orm(projection(
    name = "CredentialPublicProjection",
    fields = [id, username, status],
    private = true
))]
#[graphql_orm(projection(
    name = "CredentialSecretProjection",
    fields = [id, secret_hash],
    private = true
))]
struct RepositoryCredential {
    #[primary_key]
    id: String,
    #[unique]
    #[filterable(type = "string")]
    #[sortable]
    #[graphql_orm(searchable(weight = "A"))]
    #[graphql_orm(min_length = 3)]
    username: String,
    #[filterable(type = "string")]
    status: String,
    #[graphql_orm(
        private,
        sensitive,
        read_policy = "credentials.secret.read",
        write_policy = "credentials.secret.write"
    )]
    secret_hash: Vec<u8>,
    #[graphql_orm(version, default = "0")]
    version: i64,
}

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[repository_entity(
    backend = "sqlite",
    table = "repository_grants",
    plural = "RepositoryGrants",
    default_sort = "user_id ASC, role ASC",
    repository_mutations = true,
    unique_composite = "user_id,role",
    upsert = "user_id,role"
)]
struct RepositoryGrant {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    user_id: String,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    role: String,
    #[filterable(type = "string")]
    scope: String,
}

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[repository_entity(
    backend = "sqlite",
    table = "repository_audit_events",
    plural = "RepositoryAuditEvents",
    default_sort = "created_at ASC",
    append_only = true
)]
struct RepositoryAuditEvent {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    kind: String,
    payload: String,
    created_at: i64,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "public_things",
    plural = "PublicThings",
    default_sort = "id ASC"
)]
struct PublicThing {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    label: String,
}

schema_roots! {
    backend: "sqlite",
    query_custom_ops: [],
    entities: [PublicThing],
}

mod graphql_equivalent {
    use super::*;

    #[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
    #[graphql_entity(
        backend = "sqlite",
        table = "equivalent_entities",
        plural = "EquivalentEntities",
        default_sort = "name ASC",
        unique_composite = "tenant_id,name"
    )]
    pub struct EquivalentEntity {
        #[primary_key]
        pub id: String,
        #[filterable(type = "string")]
        #[sortable]
        pub tenant_id: String,
        #[filterable(type = "string")]
        #[sortable]
        pub name: String,
    }
}

mod repository_equivalent {
    use super::*;

    #[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
    #[repository_entity(
        backend = "sqlite",
        table = "equivalent_entities",
        plural = "EquivalentEntities",
        default_sort = "name ASC",
        unique_composite = "tenant_id,name"
    )]
    pub struct EquivalentEntity {
        #[primary_key]
        pub id: String,
        #[filterable(type = "string")]
        #[sortable]
        pub tenant_id: String,
        #[filterable(type = "string")]
        #[sortable]
        pub name: String,
    }
}

#[derive(Clone)]
struct AllowRepositoryEntities;

impl EntityPolicy<SqliteBackend> for AllowRepositoryEntities {
    fn can_access_entity<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<SqliteBackend>,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _kind: EntityAccessKind,
        _surface: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

#[derive(Clone)]
struct DenyRepositoryRows;

impl RowPolicy<SqliteBackend> for DenyRepositoryRows {
    fn can_read_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<SqliteBackend>,
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
        _db: &'a Database<SqliteBackend>,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _surface: EntityAccessSurface,
        _row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
}

#[derive(Clone)]
struct RepositoryFields(bool);

impl FieldPolicy<SqliteBackend> for RepositoryFields {
    fn can_read_field<'a>(
        &'a self,
        _ctx: &'a async_graphql::Context<'_>,
        _db: &'a Database<SqliteBackend>,
        _entity_name: &'static str,
        _field_name: &'static str,
        _policy_key: Option<&'static str>,
        _record: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }

    fn can_write_field<'a>(
        &'a self,
        _ctx: &'a async_graphql::Context<'_>,
        _db: &'a Database<SqliteBackend>,
        _entity_name: &'static str,
        _field_name: &'static str,
        _policy_key: Option<&'static str>,
        _record: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _value: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }

    fn can_read_repository_field<'a>(
        &'a self,
        _access: Option<AccessContext<'a>>,
        _db: &'a Database<SqliteBackend>,
        _entity_name: &'static str,
        _field_name: &'static str,
        policy_key: Option<&'static str>,
        _record: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(self.0 || policy_key.is_none()) })
    }

    fn can_write_repository_field<'a>(
        &'a self,
        _access: Option<AccessContext<'a>>,
        _db: &'a Database<SqliteBackend>,
        _entity_name: &'static str,
        _field_name: &'static str,
        policy_key: Option<&'static str>,
        _record: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _value: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(self.0 || policy_key.is_none()) })
    }
}

#[derive(Clone, Default)]
struct RecordingRepositoryHook {
    events: Arc<Mutex<Vec<MutationEvent>>>,
}

impl MutationHook<SqliteBackend> for RecordingRepositoryHook {
    fn on_mutation<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _mutation: &'a mut MutationContext<'_, SqliteBackend>,
        event: &'a MutationEvent,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            self.events.lock().expect("hook lock").push(event.clone());
            Ok(())
        })
    }
}

async fn database() -> graphql_orm::Result<Database<SqliteBackend>> {
    let mut database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    database.set_authorization_mode(AuthorizationMode::ExplicitPolicyForAllExposedOperations);
    database.set_entity_policy(AllowRepositoryEntities);
    database.set_field_policy(RepositoryFields(true));
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "repository-only-init",
            "repository-only test schema",
            &[
                RepositoryCredential::metadata(),
                RepositoryGrant::metadata(),
                RepositoryAuditEvent::metadata(),
                PublicThing::metadata(),
            ],
        )
        .await?;
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    Ok(database)
}

#[tokio::test]
async fn declared_repository_field_policy_fails_closed_without_provider() -> graphql_orm::Result<()>
{
    let mut database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    database.set_authorization_mode(AuthorizationMode::DeclaredPoliciesRequired);
    database.set_entity_policy(AllowRepositoryEntities);
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "repository-policy-init",
            "repository field policy",
            &[RepositoryCredential::metadata()],
        )
        .await?;
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    let error = match RepositoryCredential::insert(
        &database,
        CreateRepositoryCredentialInput {
            username: "denied".to_string(),
            status: "pending".to_string(),
            secret_hash: vec![1, 2, 3],
        },
    )
    .await
    {
        Ok(_) => panic!("declared repository field policy must require a provider"),
        Err(error) => error,
    };
    assert_eq!(
        OrmPublicError::from(error).code,
        OrmErrorCode::AuthorizationMisconfigured
    );
    Ok(())
}

#[test]
fn repository_only_generated_inputs_are_plain_and_redacted() {
    let create = CreateRepositoryCredentialInput {
        username: "alice".to_string(),
        status: "active".to_string(),
        secret_hash: vec![1, 2, 3],
    };
    let rendered = format!("{create:?}");
    assert!(rendered.contains("[redacted]"));
    assert!(!rendered.contains("1, 2, 3"));

    let projection = CredentialPublicProjection {
        id: "credential-1".to_string(),
        username: "alice".to_string(),
        status: "active".to_string(),
    };
    assert_eq!(projection.username, "alice");

    let secret_projection = CredentialSecretProjection {
        id: "credential-1".to_string(),
        secret_hash: vec![4, 5, 6],
    };
    let rendered = format!("{secret_projection:?}");
    assert!(rendered.contains("[redacted]"));
    assert!(!rendered.contains("4, 5, 6"));

    let update = UpdateRepositoryCredentialInput {
        username: None,
        status: None,
        secret_hash: Some(vec![7, 8, 9]),
    };
    let rendered = format!("{update:?}");
    assert!(rendered.contains("[redacted]"));
    assert!(!rendered.contains("7, 8, 9"));
}

#[test]
fn repository_only_mode_does_not_change_storage_metadata_or_hashes() {
    let graphql = SchemaModel::from_entities(&[graphql_equivalent::EquivalentEntity::metadata()]);
    let repository =
        SchemaModel::from_entities(&[repository_equivalent::EquivalentEntity::metadata()]);
    assert_eq!(graphql, repository);
    assert_eq!(graphql.stable_hash(), repository.stable_hash());
}

#[tokio::test]
async fn repository_only_mode_reopens_without_migration_or_validation_drift()
-> graphql_orm::Result<()> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    let graphql = graphql_equivalent::EquivalentEntity::metadata();
    let repository = repository_equivalent::EquivalentEntity::metadata();
    let initial = database
        .schema()
        .plan_migration_to_entities("equivalent-v1", "graphql declaration", &[graphql])
        .await?;
    database
        .schema()
        .apply_migration(&initial, ApplyOptions::default())
        .await?;

    let validation = database
        .schema()
        .validate_against_entities(&[repository])
        .await?;
    assert!(!validation.has_errors(), "{:#?}", validation.diagnostics);
    let replan = database
        .schema()
        .plan_migration_to_entities("equivalent-v2", "repository declaration", &[repository])
        .await?;
    assert!(replan.steps.is_empty());
    assert!(replan.statements.is_empty());
    assert_eq!(initial.target_schema_hash, replan.target_schema_hash);
    Ok(())
}

#[tokio::test]
async fn repository_only_crud_projection_transaction_cas_and_policy_are_shared()
-> graphql_orm::Result<()> {
    let mut database = database().await?;
    let hook = RecordingRepositoryHook::default();
    database.set_mutation_hook(hook.clone());
    let mut changed = database
        .ensure_event_sender::<RepositoryCredentialChangedEvent>()
        .subscribe();
    let created = RepositoryCredential::insert(
        &database,
        CreateRepositoryCredentialInput {
            username: "alice".to_string(),
            status: "pending".to_string(),
            secret_hash: vec![0xde, 0xad, 0xbe, 0xef],
        },
    )
    .await?;
    assert_eq!(created.version, 0);
    let changed = changed.recv().await.expect("post-commit event");
    assert_eq!(changed.id, created.id);
    let hook_events = hook.events.lock().expect("hook lock").clone();
    let after = hook_events
        .iter()
        .find_map(|event| event.after_state.as_ref())
        .expect("after mutation state");
    assert_eq!(after.as_json()["secretHash"], "[redacted]");
    assert!(
        after.downcast_ref::<RepositoryCredential>().is_none(),
        "redacted hook state must not retain the original sensitive entity"
    );

    let loaded = RepositoryCredential::query(&database)
        .filter(RepositoryCredentialWhereInput {
            username: Some(StringFilter {
                eq: Some("alice".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        })
        .default_order()
        .fetch_optional_one()
        .await?
        .expect("credential exists");
    assert_eq!(loaded.secret_hash, vec![0xde, 0xad, 0xbe, 0xef]);
    let search_hits = RepositoryCredential::search_db(
        &database,
        SearchInput {
            query: "alice".to_string(),
            mode: Some(SearchMode::Plain),
            min_score: None,
        },
    )
    .fetch_all()
    .await?;
    assert_eq!(search_hits.len(), 1);
    assert_eq!(search_hits[0].entity.id, created.id);
    let keyset = RepositoryCredential::keyset_page(
        &database,
        RepositoryCredentialWhereInput::default(),
        KeysetPageInput {
            limit: Some(1),
            ..Default::default()
        },
    )
    .await
    .expect("repository-only keyset page");
    assert_eq!(keyset.edges.len(), 1);

    let projected = CredentialPublicProjection::find_by_id(&database, &created.id)
        .await?
        .expect("projection exists");
    assert_eq!(projected.status, "pending");
    database.set_field_policy(RepositoryFields(false));
    assert!(
        CredentialPublicProjection::find_by_id(&database, &created.id)
            .await?
            .is_some(),
        "a projection excluding the protected field must not authorize or select it"
    );
    assert!(
        RepositoryCredential::find_by_id(&database, &created.id)
            .await
            .is_err(),
        "a full-entity read must authorize every selected persisted field"
    );
    assert!(
        RepositoryCredential::update_by_id(
            &database,
            &created.id,
            UpdateRepositoryCredentialInput {
                username: None,
                status: None,
                secret_hash: Some(vec![0]),
            },
        )
        .await
        .is_err(),
        "a declared repository write policy must gate private inputs"
    );
    database.set_field_policy(RepositoryFields(true));

    let id = created.id.clone();
    let transaction_result = database
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                let outcome = tx
                    .compare_and_swap::<RepositoryCredential>(
                        &id,
                        0,
                        RepositoryCredentialWhereInput {
                            status: Some(StringFilter {
                                eq: Some("pending".to_string()),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                        UpdateRepositoryCredentialInput {
                            username: None,
                            status: Some("active".to_string()),
                            secret_hash: None,
                        },
                    )
                    .await
                    .map_err(OrmPublicError::from)?;
                let own_write = CredentialPublicProjection::find_by_id_in(tx, &id)
                    .await
                    .map_err(OrmPublicError::from)?;
                Ok((outcome, own_write))
            })
        })
        .await
        .expect("state-machine transaction commits");
    assert!(matches!(
        transaction_result.0,
        ConditionalUpdateOutcome::Updated(_)
    ));
    assert_eq!(
        transaction_result.1.expect("own write visible").status,
        "active"
    );

    let grant = database
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                let inserted = tx
                    .insert::<RepositoryGrant>(CreateRepositoryGrantInput {
                        user_id: "user-1".to_string(),
                        role: "admin".to_string(),
                        scope: "records.read".to_string(),
                    })
                    .await
                    .map_err(OrmPublicError::from)?;
                let key = RepositoryGrantKey {
                    user_id: inserted.user_id.clone(),
                    role: inserted.role.clone(),
                };
                let visible = tx
                    .find_by_key::<RepositoryGrant>(&key)
                    .await
                    .map_err(OrmPublicError::from)?;
                Ok(visible)
            })
        })
        .await
        .expect("composite repository transaction commits")
        .expect("grant visible to own transaction");
    assert_eq!(grant.scope, "records.read");

    let audit = RepositoryAuditEvent::insert(
        &database,
        CreateRepositoryAuditEventInput {
            kind: "credential.activated".to_string(),
            payload: "redacted".to_string(),
        },
    )
    .await?;
    assert_eq!(audit.kind, "credential.activated");

    let constraint_failure = RepositoryCredential::insert(
        &database,
        CreateRepositoryCredentialInput {
            username: "xy".to_string(),
            status: "pending".to_string(),
            secret_hash: vec![9],
        },
    )
    .await;
    assert!(constraint_failure.is_err());
    assert!(
        RepositoryCredential::query(&database)
            .filter(RepositoryCredentialWhereInput {
                username: Some(StringFilter {
                    eq: Some("xy".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .fetch_first()
            .await?
            .is_none()
    );

    let rollback = database
        .transaction(TransactionMode::StateMachine, |transaction| {
            Box::pin(async move {
                transaction
                    .insert::<RepositoryCredential>(CreateRepositoryCredentialInput {
                        username: "rolled-back".to_string(),
                        status: "pending".to_string(),
                        secret_hash: vec![4, 5, 6],
                    })
                    .await
                    .map_err(OrmPublicError::from)?;
                Err::<(), _>(OrmPublicError::new(OrmErrorCode::Conflict))
            })
        })
        .await;
    assert!(rollback.is_err());
    assert!(
        RepositoryCredential::query(&database)
            .filter(RepositoryCredentialWhereInput {
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

    RepositoryCredential::insert(
        &database,
        CreateRepositoryCredentialInput {
            username: "bob".to_string(),
            status: "active".to_string(),
            secret_hash: vec![7],
        },
    )
    .await?;
    database.set_pagination_config(PaginationConfig::secure().with_max_limit(Some(1)));
    assert_eq!(RepositoryCredential::find_all(&database).await?.len(), 1);
    let ambiguous = RepositoryCredential::query(&database)
        .filter(RepositoryCredentialWhereInput::default())
        .fetch_optional_one()
        .await;
    assert!(
        ambiguous.is_err(),
        "optional-one must fetch one look-ahead row"
    );

    database.set_row_policy(DenyRepositoryRows);
    let denied =
        RepositoryCredential::find_many(&database, RepositoryCredentialWhereInput::default())
            .await?;
    assert!(denied.is_empty(), "repository query must apply row policy");
    let denied_search = RepositoryCredential::search_db(
        &database,
        SearchInput {
            query: "alice".to_string(),
            mode: Some(SearchMode::Plain),
            min_score: None,
        },
    )
    .fetch_all()
    .await?;
    assert!(
        denied_search.is_empty(),
        "repository search must apply row policy"
    );
    let denied_keyset = RepositoryCredential::keyset_page(
        &database,
        RepositoryCredentialWhereInput::default(),
        KeysetPageInput {
            limit: Some(1),
            ..Default::default()
        },
    )
    .await;
    assert!(
        denied_keyset.is_err(),
        "keyset reads must fail closed when a row policy cannot be rendered into SQL"
    );
    Ok(())
}

#[tokio::test]
async fn repository_only_types_are_absent_from_graphql_sdl() -> graphql_orm::Result<()> {
    let database = database().await?;
    let schema = schema_builder(database).finish();
    let sdl = schema.sdl();
    assert!(sdl.contains("type PublicThing"));
    for forbidden in [
        "RepositoryCredential",
        "RepositoryCredentialWhereInput",
        "CreateRepositoryCredentialInput",
        "CredentialPublicProjection",
        "CredentialSecretProjection",
        "RepositoryGrant",
        "RepositoryAuditEvent",
    ] {
        assert!(!sdl.contains(forbidden), "unexpected SDL type: {forbidden}");
    }
    Ok(())
}
