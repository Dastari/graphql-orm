//! A minimal external-style entity: a generated key, a server-owned
//! `skip_input` timestamp and a relation, served without generated mutations.
use graphql_orm::prelude::*;

#[derive(
    GraphQLEntity,
    GraphQLOperations,
    GraphQLRelations,
    graphql_orm::async_graphql::SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[graphql_entity(table = "licences", plural = "Licences")]
struct Licence {
    #[primary_key]
    #[sortable]
    id: String,
    #[graphql_orm(skip_input)]
    updated_timestamp: String,
    #[graphql(skip)]
    #[serde(default)]
    #[relation(target = "Seat", from = "id", to = "licence_id", multiple, emit_fk = false)]
    seats: Vec<Seat>,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "seats", plural = "Seats")]
struct Seat {
    #[primary_key]
    #[sortable]
    id: String,
    licence_id: String,
}

impl BatchLoadEntity for Seat {
    fn batch_column() -> &'static str {
        "id"
    }
    fn batch_key_from_row(row: &graphql_orm::DbRow) -> graphql_orm::Result<String> {
        use graphql_orm::sqlx::Row;
        row.try_get("id")
    }
}

schema_roots! {
    generated_mutations: "none",
    query_custom_ops: [],
    entities: [Licence, Seat],
}

#[allow(dead_code)]
async fn private_writes(db: &Database) -> graphql_orm::Result<()> {
    let licence = Licence::insert(
        db,
        CreateLicenceInput {
            updated_timestamp: "t1".into(),
        },
    )
    .await?;
    Licence::update_by_id(
        db,
        &licence.id,
        UpdateLicenceInput {
            updated_timestamp: Some("t2".into()),
        },
    )
    .await?;
    Ok(())
}

fn main() {}
