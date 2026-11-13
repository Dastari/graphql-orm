#![cfg(feature = "sqlite")]

use graphql_orm::prelude::*;

fn reveal(value: String) -> String {
    value.to_uppercase()
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "read_transform_records")]
struct ReadTransformRecord {
    #[primary_key]
    #[sortable]
    id: String,
    #[transform(read = "reveal")]
    value: String,
}

#[tokio::test]
async fn read_transform_runs_without_changing_stored_values() -> graphql_orm::Result<()> {
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    let plan = db
        .schema()
        .plan_migration_to_entities(
            "read-transform-init",
            "read transform",
            &[ReadTransformRecord::metadata()],
        )
        .await?;
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    let created = ReadTransformRecord::insert(
        &db,
        CreateReadTransformRecordInput {
            value: "plain".to_owned(),
        },
    )
    .await?;
    let stored: String =
        graphql_orm::sqlx::query_scalar("SELECT value FROM read_transform_records WHERE id = ?")
            .bind(&created.id)
            .fetch_one(db.pool())
            .await?;
    assert_eq!(stored, "plain");
    let fetched = ReadTransformRecord::find_by_id(&db, &created.id)
        .await?
        .expect("stored row");
    assert_eq!(fetched.value, "PLAIN");
    Ok(())
}
