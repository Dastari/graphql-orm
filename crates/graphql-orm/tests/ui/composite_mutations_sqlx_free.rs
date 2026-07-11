use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, Clone, Debug)]
#[graphql_entity(
    backend = "sqlite",
    table = "private_grants",
    plural = "PrivateGrants",
    repository_mutations = true,
    default_sort = "subject_id ASC, grant ASC",
    unique_composite = "subject_id,grant",
    upsert = "subject_id,grant"
)]
struct PrivateGrant {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    subject_id: String,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    grant: String,
    #[filterable(type = "number")]
    consumed_at: Option<i64>,
}

async fn host_code(database: &graphql_orm::db::Database) -> graphql_orm::Result<()> {
    let key = PrivateGrantKey {
        subject_id: "user".to_string(),
        grant: "role".to_string(),
    };
    let _ = PrivateGrant::find_by_key(database, &key).await?;
    let _ = PrivateGrant::insert_if_absent(
        database,
        CreatePrivateGrantInput {
            subject_id: key.subject_id.clone(),
            grant: key.grant.clone(),
            consumed_at: None,
        },
    )
    .await?;
    let _ = PrivateGrant::update_if(
        database,
        &key,
        PrivateGrantWhereInput {
            consumed_at: Some(IntFilter {
                is_null: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        },
        UpdatePrivateGrantInput {
            consumed_at: Some(Some(1)),
        },
    )
    .await?;
    Ok(())
}

fn main() {
    let _ = host_code;
}
