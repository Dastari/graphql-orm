use graphql_orm::prelude::*;

#[derive(GraphQLEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(table = "orm_spatial_places_ui", backend = "sqlite")]
struct SqliteSpatialPlace {
    #[primary_key]
    #[filterable(type = "uuid")]
    #[sortable]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,

    #[graphql_orm(spatial(kind = "geometry", geometry_type = "Point", srid = 4326))]
    #[filterable(type = "spatial")]
    pub location: graphql_orm::serde_json::Value,
}

fn main() {}
