#![cfg(any(feature = "sqlite", feature = "postgres"))]

use graphql_orm::prelude::*;

#[derive(
    GraphQLEntity, GraphQLOperations, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize,
)]
#[graphql_entity(
    table = "binary_key_records",
    plural = "BinaryKeyRecords",
    upsert = "digest",
    keyset = "digest asc"
)]
struct BinaryKeyRecord {
    #[primary_key]
    #[filterable(type = "bytes")]
    #[sortable]
    #[graphql_orm(private, auto_generated = false, min_length = 32, max_length = 32)]
    digest: Vec<u8>,
    #[sortable]
    payload: String,
    #[graphql_orm(version, default = "0")]
    #[filterable(type = "number")]
    version: i64,
}

schema_roots! {
    query_custom_ops: [],
    entities: [BinaryKeyRecord],
}

#[derive(Clone)]
struct BinaryRowPolicy;

impl graphql_orm::graphql::orm::RowPolicy for BinaryRowPolicy {
    fn can_read_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _surface: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok(row
                .downcast_ref::<BinaryKeyRecord>()
                .is_none_or(|record| record.digest.len() == 32))
        })
    }

    fn can_write_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _surface: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok(row
                .downcast_ref::<BinaryKeyRecord>()
                .is_none_or(|record| record.digest.len() == 32))
        })
    }
}

#[cfg(feature = "sqlite")]
async fn database() -> graphql_orm::Result<Database<SqliteBackend>> {
    Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await
}

#[cfg(feature = "postgres")]
async fn database() -> graphql_orm::Result<Database<PostgresBackend>> {
    let url = std::env::var("TEST_DATABASE_URL")
        .map_err(|error| graphql_orm::Error::Configuration(error.to_string().into()))?;
    let database = Database::<PostgresBackend>::connect_postgres(url).await?;
    graphql_orm::sqlx::query("DROP TABLE IF EXISTS binary_key_records CASCADE")
        .execute(database.pool())
        .await?;
    Ok(database)
}

#[tokio::test]
async fn binary_keys_remain_bytes_across_repository_transaction_cas_and_keysets()
-> Result<(), Box<dyn std::error::Error>> {
    if cfg!(feature = "postgres") && std::env::var("TEST_DATABASE_URL").is_err() {
        return Ok(());
    }
    let mut database = database().await?;
    database.set_row_policy(BinaryRowPolicy);
    let entities = [BinaryKeyRecord::metadata()];
    let plan = database
        .schema()
        .plan_migration_to_entities_with_options(
            format!("binary-keys-{}", graphql_orm::uuid::Uuid::new_v4()),
            "binary keys",
            &entities,
            PlanOptions::managed_tables_only(),
        )
        .await?;
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;

    let digest = vec![0xabu8; 32];
    let created = BinaryKeyRecord::insert(
        &database,
        CreateBinaryKeyRecordInput {
            digest: digest.clone(),
            payload: "created".to_string(),
        },
    )
    .await?;
    assert_eq!(created.digest, digest);
    assert_eq!(
        BinaryKeyRecord::find_by_id(&database, &digest)
            .await?
            .expect("binary key row")
            .payload,
        "created"
    );

    let cas = BinaryKeyRecord::compare_and_swap(
        &database,
        &digest,
        0,
        BinaryKeyRecordWhereInput::default(),
        UpdateBinaryKeyRecordInput {
            payload: Some("cas".to_string()),
        },
    )
    .await?;
    assert!(matches!(
        cas,
        ConditionalUpdateOutcome::Updated(BinaryKeyRecord { version: 1, .. })
    ));

    let digest_for_transaction = digest.clone();
    database
        .transaction(TransactionMode::StateMachine, |transaction| {
            Box::pin(async move {
                let visible = transaction
                    .find_by_id::<BinaryKeyRecord>(&digest_for_transaction)
                    .await?
                    .expect("transaction-visible binary row");
                assert_eq!(visible.payload, "cas");
                let outcome = transaction
                    .upsert::<BinaryKeyRecord>(CreateBinaryKeyRecordInput {
                        digest: digest_for_transaction,
                        payload: "upserted".to_string(),
                    })
                    .await?;
                assert_eq!(outcome.entity.payload, "upserted");
                Ok::<_, OrmPublicError>(())
            })
        })
        .await?;

    let page = BinaryKeyRecord::keyset_page(
        &database,
        BinaryKeyRecordWhereInput::default(),
        KeysetPageInput {
            limit: Some(1),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(page.edges[0].node.digest, digest);
    assert!(page.edges[0].cursor.starts_with("gomk1."));

    let invalid = BinaryKeyRecord::insert(
        &database,
        CreateBinaryKeyRecordInput {
            digest: vec![1; 31],
            payload: "invalid".to_string(),
        },
    )
    .await
    .expect_err("managed length check rejects non-digest key");
    assert_eq!(
        OrmPublicError::from_sqlx(&invalid).code,
        OrmErrorCode::ConstraintViolation
    );

    assert!(BinaryKeyRecord::delete_by_id(&database, &digest).await?);
    assert!(
        BinaryKeyRecord::find_by_id(&database, &digest)
            .await?
            .is_none()
    );

    #[cfg(feature = "postgres")]
    graphql_orm::sqlx::query("DROP TABLE IF EXISTS binary_key_records CASCADE")
        .execute(database.pool())
        .await?;
    Ok(())
}

#[tokio::test]
async fn private_binary_key_and_unsafe_graphql_upsert_are_not_exposed() {
    let schema = schema_builder(Database::new(
        #[cfg(feature = "sqlite")]
        graphql_orm::sqlx::SqlitePool::connect_lazy("sqlite::memory:").unwrap(),
        #[cfg(feature = "postgres")]
        graphql_orm::sqlx::PgPool::connect_lazy("postgres://unused:unused@localhost/unused")
            .unwrap(),
    ))
    .finish();
    let sdl = schema.sdl();
    let create_input = sdl
        .split("input CreateBinaryKeyRecordInput")
        .nth(1)
        .and_then(|value| value.split('}').next())
        .unwrap_or_default();
    assert!(!create_input.contains("digest:"));
    assert!(!sdl.contains("upsertBinaryKeyRecord"));
}
