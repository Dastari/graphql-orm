use graphql_orm::prelude::*;

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "typed_grants",
    plural = "TypedGrants",
    repository_mutations = true,
    unique_composite = "subject_id,grant_name",
    default_sort = "subject_id ASC, grant_name ASC",
    upsert = "subject_id,grant_name",
    write_policy = "grants.write"
)]
struct TypedGrant {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    subject_id: String,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    grant_name: String,
    #[filterable(type = "string")]
    value: String,
    #[filterable(type = "number")]
    consumed_at: Option<i64>,
}

schema_roots! {
    query_custom_ops: [],
    entities: [TypedGrant],
}

#[derive(Clone)]
struct AllowEntityPolicy;

impl EntityPolicy for AllowEntityPolicy {
    fn can_access_entity<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _kind: EntityAccessKind,
        _surface: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

#[derive(Clone)]
#[cfg(feature = "sqlite")]
struct DenyRows;

#[cfg(feature = "sqlite")]
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

async fn database() -> Result<graphql_orm::db::Database, Box<dyn std::error::Error>> {
    #[cfg(feature = "sqlite")]
    {
        Ok(graphql_orm::db::Database::connect_sqlite("sqlite::memory:").await?)
    }
    #[cfg(feature = "postgres")]
    {
        let url = std::env::var("TEST_DATABASE_URL")?;
        Ok(graphql_orm::db::Database::connect_postgres(&url).await?)
    }
}

#[tokio::test]
async fn composite_repository_and_transaction_crud_is_typed()
-> Result<(), Box<dyn std::error::Error>> {
    if cfg!(feature = "postgres") && std::env::var("TEST_DATABASE_URL").is_err() {
        return Ok(());
    }
    let mut database = database().await?;
    database.set_entity_policy(AllowEntityPolicy);
    #[cfg(feature = "sqlite")]
    {
        let entities = [TypedGrant::metadata()];
        let plan = database
            .schema()
            .plan_migration_to_entities(
                "composite-mutations",
                "typed composite mutations",
                &entities,
            )
            .await?;
        database
            .schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await?;
    }
    #[cfg(feature = "postgres")]
    {
        graphql_orm::sqlx::query("DROP TABLE IF EXISTS typed_grants CASCADE")
            .execute(database.pool())
            .await?;
        graphql_orm::sqlx::query(
            "CREATE TABLE typed_grants (
                subject_id TEXT NOT NULL,
                grant_name TEXT NOT NULL,
                value TEXT NOT NULL,
                consumed_at BIGINT NULL,
                PRIMARY KEY (subject_id, grant_name),
                UNIQUE (subject_id, grant_name)
            )",
        )
        .execute(database.pool())
        .await?;
    }

    let key = TypedGrantKey {
        subject_id: "subject'; DROP TABLE typed_grants; --".to_string(),
        grant_name: "admin".to_string(),
    };
    let inserted = TypedGrant::insert(
        &database,
        CreateTypedGrantInput {
            subject_id: key.subject_id.clone(),
            grant_name: key.grant_name.clone(),
            value: "initial".to_string(),
            consumed_at: None,
        },
    )
    .await?;
    assert_eq!(inserted.subject_id, key.subject_id);
    assert_eq!(
        TypedGrant::find_by_key(&database, &key).await?,
        Some(inserted.clone())
    );

    let updated = TypedGrant::update_by_key(
        &database,
        &key,
        UpdateTypedGrantInput {
            value: Some("updated".to_string()),
            ..Default::default()
        },
    )
    .await?
    .expect("updated row");
    assert_eq!(updated.value, "updated");

    let own_write = database
        .transaction(TransactionMode::StateMachine, |transaction| {
            let key = key.clone();
            Box::pin(async move {
                transaction
                    .update_by_key::<TypedGrant>(
                        &key,
                        UpdateTypedGrantInput {
                            value: Some("transaction".to_string()),
                            ..Default::default()
                        },
                    )
                    .await
                    .map_err(OrmPublicError::from)?;
                transaction
                    .find_by_key::<TypedGrant>(&key)
                    .await
                    .map_err(OrmPublicError::from)
            })
        })
        .await?
        .expect("transaction read sees update");
    assert_eq!(own_write.value, "transaction");

    let rollback_key = TypedGrantKey {
        subject_id: "rollback-subject".to_string(),
        grant_name: "rollback-role".to_string(),
    };
    let rollback = database
        .transaction(TransactionMode::StateMachine, |transaction| {
            let rollback_key = rollback_key.clone();
            Box::pin(async move {
                transaction
                    .insert::<TypedGrant>(CreateTypedGrantInput {
                        subject_id: rollback_key.subject_id,
                        grant_name: rollback_key.grant_name,
                        value: "must-roll-back".to_string(),
                        consumed_at: None,
                    })
                    .await
                    .map_err(OrmPublicError::from)?;
                Err::<(), _>(OrmPublicError::new(OrmErrorCode::Conflict))
            })
        })
        .await;
    assert!(rollback.is_err());
    assert!(
        TypedGrant::find_by_key(&database, &rollback_key)
            .await?
            .is_none()
    );

    let constraint_key = TypedGrantKey {
        subject_id: "constraint-subject".to_string(),
        grant_name: "duplicate".to_string(),
    };
    let constraint_failure = database
        .transaction(TransactionMode::StateMachine, |transaction| {
            let constraint_key = constraint_key.clone();
            Box::pin(async move {
                for _ in 0..2 {
                    transaction
                        .insert::<TypedGrant>(CreateTypedGrantInput {
                            subject_id: constraint_key.subject_id.clone(),
                            grant_name: constraint_key.grant_name.clone(),
                            value: "duplicate".to_string(),
                            consumed_at: None,
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                }
                Ok(())
            })
        })
        .await;
    assert!(constraint_failure.is_err());
    assert!(
        TypedGrant::find_by_key(&database, &constraint_key)
            .await?
            .is_none()
    );

    let second_input = || CreateTypedGrantInput {
        subject_id: key.subject_id.clone(),
        grant_name: "viewer".to_string(),
        value: "viewer-initial".to_string(),
        consumed_at: None,
    };
    let second = match TypedGrant::insert_if_absent(&database, second_input()).await? {
        InsertIfAbsentOutcome::Inserted(entity) => entity,
        InsertIfAbsentOutcome::AlreadyPresent(_) => panic!("first insert-if-absent must insert"),
    };
    assert_eq!(second.grant_name, "viewer");
    assert!(matches!(
        TypedGrant::insert_if_absent(&database, second_input()).await?,
        InsertIfAbsentOutcome::AlreadyPresent(_)
    ));

    let upserted = TypedGrant::upsert(
        &database,
        CreateTypedGrantInput {
            value: "viewer-upserted".to_string(),
            ..second_input()
        },
    )
    .await?;
    assert_eq!(upserted.action, ChangeAction::Updated);
    assert_eq!(upserted.entity.value, "viewer-upserted");

    let second_key = TypedGrantKey {
        subject_id: key.subject_id.clone(),
        grant_name: "viewer".to_string(),
    };
    let consumed = TypedGrant::update_if(
        &database,
        &second_key,
        TypedGrantWhereInput {
            consumed_at: Some(IntFilter {
                is_null: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        },
        UpdateTypedGrantInput {
            consumed_at: Some(Some(123)),
            ..Default::default()
        },
    )
    .await?;
    assert!(matches!(consumed, PredicateUpdateOutcome::Updated(_)));
    let conflict = TypedGrant::update_if(
        &database,
        &second_key,
        TypedGrantWhereInput {
            consumed_at: Some(IntFilter {
                is_null: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        },
        UpdateTypedGrantInput {
            value: Some("must-not-write".to_string()),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(conflict, PredicateUpdateOutcome::PredicateConflict);

    let subject_filter = || TypedGrantWhereInput {
        subject_id: Some(StringFilter {
            eq: Some(key.subject_id.clone()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let limited = TypedGrant::update_where_bounded(
        &database,
        subject_filter(),
        UpdateTypedGrantInput {
            value: Some("bounded".to_string()),
            ..Default::default()
        },
        MutationLimit::new(1)?,
    )
    .await?;
    assert_eq!(
        limited,
        BoundedMutationOutcome::LimitExceeded { maximum: 1 }
    );
    let applied = TypedGrant::update_where_bounded(
        &database,
        subject_filter(),
        UpdateTypedGrantInput {
            value: Some("bounded".to_string()),
            ..Default::default()
        },
        MutationLimit::new(2)?,
    )
    .await?;
    assert_eq!(applied, BoundedMutationOutcome::Applied { affected: 2 });

    let deleted =
        TypedGrant::delete_where_bounded(&database, subject_filter(), MutationLimit::new(2)?)
            .await?;
    assert_eq!(deleted, BoundedMutationOutcome::Applied { affected: 2 });

    assert!(TypedGrant::find_by_key(&database, &key).await?.is_none());
    #[cfg(feature = "postgres")]
    graphql_orm::sqlx::query("DROP TABLE typed_grants CASCADE")
        .execute(database.pool())
        .await?;
    Ok(())
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn composite_repository_opt_in_adds_no_graphql_mutations_and_strict_auth_fails_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let schema_database = database().await?;
    let schema = schema_builder(schema_database.clone()).finish();
    let sdl = schema.sdl();
    assert!(!sdl.contains("createTypedGrant"));
    assert!(!sdl.contains("updateTypedGrant"));
    assert!(!sdl.contains("deleteTypedGrant"));
    assert!(!sdl.contains("upsertTypedGrant"));

    let strict =
        schema_database.with_authorization_mode(AuthorizationMode::DeclaredPoliciesRequired);
    let entities = [TypedGrant::metadata()];
    let plan = strict
        .schema()
        .plan_migration_to_entities("strict-composite", "strict composite auth", &entities)
        .await?;
    strict
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    let denied = TypedGrant::insert(
        &strict,
        CreateTypedGrantInput {
            subject_id: "strict".to_string(),
            grant_name: "denied".to_string(),
            value: "denied".to_string(),
            consumed_at: None,
        },
    )
    .await
    .expect_err("declared write policy without provider must fail closed");
    assert!(denied.to_string().to_ascii_lowercase().contains("authoriz"));

    let mut row_denied = database().await?;
    row_denied.set_entity_policy(AllowEntityPolicy);
    row_denied.set_row_policy(DenyRows);
    let plan = row_denied
        .schema()
        .plan_migration_to_entities("row-denied", "row policy denial", &entities)
        .await?;
    row_denied
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    TypedGrant::insert(
        &row_denied,
        CreateTypedGrantInput {
            subject_id: "row".to_string(),
            grant_name: "denied".to_string(),
            value: "denied".to_string(),
            consumed_at: None,
        },
    )
    .await
    .expect_err("row write policy must not be bypassed");
    assert!(
        TypedGrant::find_by_key(
            &row_denied,
            &TypedGrantKey {
                subject_id: "row".to_string(),
                grant_name: "denied".to_string(),
            }
        )
        .await?
        .is_none()
    );
    Ok(())
}

#[cfg(feature = "postgres")]
#[tokio::test]
async fn postgres_composite_mutations_honor_transaction_auth_and_rls()
-> Result<(), Box<dyn std::error::Error>> {
    use std::str::FromStr;

    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        return Ok(());
    };
    let owner = graphql_orm::sqlx::PgPool::connect(&url).await?;
    graphql_orm::sqlx::query("DROP TABLE IF EXISTS typed_grants CASCADE")
        .execute(&owner)
        .await?;
    graphql_orm::sqlx::query(
        "CREATE TABLE typed_grants (
            subject_id TEXT NOT NULL,
            grant_name TEXT NOT NULL,
            value TEXT NOT NULL,
            consumed_at BIGINT NULL,
            PRIMARY KEY (subject_id, grant_name),
            UNIQUE (subject_id, grant_name)
        )",
    )
    .execute(&owner)
    .await?;
    graphql_orm::sqlx::query("ALTER TABLE typed_grants ENABLE ROW LEVEL SECURITY")
        .execute(&owner)
        .await?;
    graphql_orm::sqlx::query("ALTER TABLE typed_grants FORCE ROW LEVEL SECURITY")
        .execute(&owner)
        .await?;
    graphql_orm::sqlx::query(
        "CREATE POLICY typed_grants_tenant ON typed_grants
         FOR ALL
         USING (subject_id = current_setting('app.tenant_id', true))
         WITH CHECK (subject_id = current_setting('app.tenant_id', true))",
    )
    .execute(&owner)
    .await?;

    let role = format!(
        "composite_writer_{}",
        graphql_orm::uuid::Uuid::new_v4().simple()
    );
    let password = "composite-test-password";
    graphql_orm::sqlx::query(&format!("CREATE ROLE {role} LOGIN PASSWORD '{password}'"))
        .execute(&owner)
        .await?;
    graphql_orm::sqlx::query(&format!("GRANT USAGE ON SCHEMA public TO {role}"))
        .execute(&owner)
        .await?;
    graphql_orm::sqlx::query(&format!(
        "GRANT SELECT, INSERT, UPDATE, DELETE ON typed_grants TO {role}"
    ))
    .execute(&owner)
    .await?;

    let options = graphql_orm::sqlx::postgres::PgConnectOptions::from_str(&url)?
        .username(&role)
        .password(password);
    let pool = graphql_orm::sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await?;
    let mut database = Database::<PostgresBackend>::new(pool.clone());
    database.set_entity_policy(AllowEntityPolicy);
    let auth = DbAuthContext {
        user_id: Some("writer".to_string()),
        subject: Some("writer".to_string()),
        tenant_id: Some("tenant-a".to_string()),
        ..Default::default()
    };
    let key = TypedGrantKey {
        subject_id: "tenant-a".to_string(),
        grant_name: "admin".to_string(),
    };
    let inserted = database
        .transaction_with_auth(TransactionMode::StateMachine, Some(&auth), |transaction| {
            let key = key.clone();
            Box::pin(async move {
                transaction
                    .insert::<TypedGrant>(CreateTypedGrantInput {
                        subject_id: key.subject_id.clone(),
                        grant_name: key.grant_name.clone(),
                        value: "allowed".to_string(),
                        consumed_at: None,
                    })
                    .await
                    .map_err(OrmPublicError::from)?;
                transaction
                    .find_by_key::<TypedGrant>(&key)
                    .await
                    .map_err(OrmPublicError::from)
            })
        })
        .await?
        .expect("own tenant row visible");
    assert_eq!(inserted.value, "allowed");

    let wrong_auth = DbAuthContext {
        tenant_id: Some("tenant-b".to_string()),
        ..auth.clone()
    };
    let hidden = database
        .transaction_with_auth(TransactionMode::Default, Some(&wrong_auth), |transaction| {
            let key = key.clone();
            Box::pin(async move {
                transaction
                    .find_by_key::<TypedGrant>(&key)
                    .await
                    .map_err(OrmPublicError::from)
            })
        })
        .await?;
    assert!(hidden.is_none());

    let denied = database
        .transaction(TransactionMode::Default, |transaction| {
            Box::pin(async move {
                transaction
                    .insert::<TypedGrant>(CreateTypedGrantInput {
                        subject_id: "tenant-a".to_string(),
                        grant_name: "without-auth".to_string(),
                        value: "denied".to_string(),
                        consumed_at: None,
                    })
                    .await
                    .map_err(OrmPublicError::from)
            })
        })
        .await;
    assert!(denied.is_err());

    drop(database);
    pool.close().await;
    graphql_orm::sqlx::query("DROP TABLE typed_grants CASCADE")
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
