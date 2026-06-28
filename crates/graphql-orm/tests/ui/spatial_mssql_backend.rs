use graphql_orm::prelude::*;

#[derive(GraphQLEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(table = "bad_spatial_places", backend = "mssql")]
struct BadSpatialPlace {
    #[primary_key]
    pub id: String,

    #[graphql_orm(spatial(kind = "geometry", geometry_type = "Point", srid = 4326))]
    #[filterable(type = "spatial")]
    pub location: graphql_orm::serde_json::Value,
}

fn main() {}
