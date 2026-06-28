use graphql_orm::prelude::*;

#[derive(GraphQLEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(table = "bad_spatial_places", backend = "postgres")]
struct BadSpatialPlace {
    #[primary_key]
    pub id: graphql_orm::uuid::Uuid,

    #[graphql_orm(spatial(kind = "geometry", geometry_type = "Point", srid = 4326))]
    #[filterable(type = "spatial")]
    pub location: String,
}

fn main() {}
